//! Covers `src/graphql/relations.rs` and what `#[expose]` emits on a relation's
//! field: a relation answers what its grant allows by a root's rule, whichever
//! alias asks, and a hand-written `#[field_resolver]` answering a wire type does
//! too. The batches below hold their rows in memory where the emitted ones read
//! `Repo`, which `tests/e2e/graphql/relations.rs` proves.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use nest_rs_authz::__private::AmbientAbility;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, OPAQUE_CLIENT_MESSAGE, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::connection::{
    Connection, ConnectionNameType, Edge, EdgeNameType, EmptyFields,
};
use nest_rs_graphql::async_graphql::dataloader::DataLoader;
use nest_rs_graphql::async_graphql::{
    Error as GqlError, Interface, Json, MergedObject, Object, OutputType, Result as GqlResult,
    SimpleObject, Union,
};
use nest_rs_graphql::{GraphqlModule, GraphqlView, dataloader, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::EndpointGuards;
use nest_rs_seaorm::graphql::{PkLoadable, RelatedTo, RelationKey, RelationPage};
use nest_rs_seaorm::{CrudService, PageCursor, SeaOrmDatabaseModule, ServiceError};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::prelude::Uuid;
use serde_json::{Value, json};

#[path = "../../harness/role_guard.rs"]
mod role_guard;

use role_guard::{RoleGuard, answer, assert_no_error_line};

pub(crate) mod writer {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Writer", graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "writers")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        #[expose]
        pub email: String,
        #[expose]
        pub nickname: Option<String>,
        #[expose]
        pub circle: i32,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod book {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Book", service = super::BooksService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "books")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub writer_id: Uuid,
        #[sea_orm(belongs_to, from = "writer_id", to = "id")]
        #[expose]
        pub writer: HasOne<super::writer::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod novel {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Novel", graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "novels")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: String,
        #[expose]
        pub isbn: String,
        #[expose]
        pub shelf_id: Uuid,
        #[sea_orm(belongs_to, from = "shelf_id", to = "id")]
        pub shelf: HasOne<super::shelf::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// A novel keyed by a column of two words, so its cursor reads a field the schema and
/// the wire spell differently.
pub(crate) mod volume {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Volume", graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "volumes")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub volume_id: Uuid,
        #[expose]
        pub title: String,
        #[expose]
        pub shelf_id: Uuid,
        #[sea_orm(belongs_to, from = "shelf_id", to = "id")]
        pub shelf: HasOne<super::shelf::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod shelf {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Shelf", service = super::ShelvesService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "shelves")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[sea_orm(has_many)]
        #[expose]
        pub novels: HasMany<super::novel::Entity>,
        #[sea_orm(has_many)]
        #[expose]
        pub volumes: HasMany<super::volume::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod secretive {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Secretive", graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "secretives")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        pub listed: bool,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// What a `BTreeSet` and a `HashSet` of the wire type ask of it.
impl PartialEq for writer::Writer {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for writer::Writer {}

impl Hash for writer::Writer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialOrd for writer::Writer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for writer::Writer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

const SECRET_EMAIL: &str = "withheld@example.test";
const SECRET_ISBN: &str = "978-withheld";
const UNLISTED_NAME: &str = "unlisted name";

fn writer_id() -> Uuid {
    Uuid::from_u128(0x0192_0000_0000_7000_8000_0000_0000_00a1)
}

fn shelf_id() -> Uuid {
    Uuid::from_u128(0x0192_0000_0000_7000_8000_0000_0000_00b1)
}

fn novel_ids() -> [Uuid; 2] {
    [
        Uuid::from_u128(0x0192_0000_0000_7000_8000_0000_0000_00c1),
        Uuid::from_u128(0x0192_0000_0000_7000_8000_0000_0000_00c2),
    ]
}

fn writer_row() -> writer::Model {
    writer::Model {
        id: writer_id(),
        name: "Ada".to_owned(),
        email: SECRET_EMAIL.to_owned(),
        nickname: Some("countess".to_owned()),
        circle: 1,
    }
}

fn lead_row() -> writer::Writer {
    writer::Writer::from(&writer_row())
}

fn unlisted_row() -> secretive::Secretive {
    secretive::Secretive::from(&secretive::Model {
        id: writer_id(),
        name: UNLISTED_NAME.to_owned(),
        listed: false,
    })
}

fn novel_rows() -> Vec<novel::Model> {
    novel_ids()
        .into_iter()
        .map(|id| novel::Model {
            id,
            title: "Sketches".to_owned(),
            isbn: SECRET_ISBN.to_owned(),
            shelf_id: shelf_id(),
        })
        .collect()
}

#[injectable]
#[derive(Default)]
pub(crate) struct BooksService;

impl CrudService for BooksService {
    type Entity = book::Entity;
}

#[injectable]
#[derive(Default)]
pub(crate) struct ShelvesService;

impl CrudService for ShelvesService {
    type Entity = shelf::Entity;
}

/// The batches standing in for the emitted ones.
#[injectable]
#[derive(Default)]
struct Rows;

#[dataloader]
impl Rows {
    async fn writers(&self, keys: &[Uuid]) -> Result<HashMap<Uuid, writer::Writer>, ServiceError> {
        let row = writer::Writer::from(&writer_row());
        Ok(keys.iter().map(|key| (*key, row.clone())).collect())
    }

    async fn novels(
        &self,
        keys: &[RelationKey<Uuid>],
    ) -> Result<HashMap<RelationKey<Uuid>, RelationPage<novel::Novel>>, ServiceError> {
        let mut pages = HashMap::with_capacity(keys.len());
        for key in keys {
            let mut edges = Vec::new();
            let rows = novel_rows();
            let after = key.after.as_ref().map(PageCursor::to_string);
            let rest = rows.iter().filter(|row| {
                after
                    .as_ref()
                    .is_none_or(|after| row.id.to_string() > *after)
            });
            for row in rest
                .clone()
                .take(usize::try_from(key.first).unwrap_or(usize::MAX))
            {
                edges.push((
                    PageCursor::from(row.id).to_string(),
                    novel::Novel::from(row),
                ));
            }
            let has_next_page = rest.count() > edges.len();
            pages.insert(
                key.clone(),
                nest_rs_seaorm::__private::relation_page(edges, has_next_page),
            );
        }
        Ok(pages)
    }

    async fn volumes(
        &self,
        keys: &[RelationKey<Uuid>],
    ) -> Result<HashMap<RelationKey<Uuid>, RelationPage<volume::Volume>>, ServiceError> {
        let row = volume::Model {
            volume_id: novel_ids()[0],
            title: "Sketches".to_owned(),
            shelf_id: shelf_id(),
        };
        Ok(keys
            .iter()
            .map(|key| {
                let edge = (
                    PageCursor::from(row.volume_id).to_string(),
                    volume::Volume::from(&row),
                );
                let page = nest_rs_seaorm::__private::relation_page(vec![edge], false);
                (key.clone(), page)
            })
            .collect())
    }
}

impl PkLoadable for writer::Entity {
    type Loader = RowsWriters;
}

impl RelatedTo<shelf::Entity> for novel::Entity {
    type Loader = RowsNovels;
}

impl RelatedTo<shelf::Entity> for volume::Entity {
    type Loader = RowsVolumes;
}

fn ability_of(role: &str) -> nest_rs_authz::Ability {
    let mut ab = AbilityBuilder::new();
    if role.is_empty() {
        ab.can(Action::Read, writer::Entity)
            .fields([writer::Column::Name]);
        // On a column the wire type leaves out: only a read could decide it.
        ab.can(Action::Read, secretive::Entity)
            .when(|p| p.eq(secretive::Column::Listed, true));
        return ab.build_visitor().expect("a valid rule set");
    }
    // Reads a secretive row whole, but for a denial only a read could decide.
    if role == "wary-visitor" {
        ab.can(Action::Read, secretive::Entity);
        ab.cannot(Action::Read, secretive::Entity)
            .when(|p| p.eq(secretive::Column::Listed, false));
        return ab.build_visitor().expect("a valid rule set");
    }
    ab.can(Action::Read, book::Entity);
    ab.can(Action::Read, shelf::Entity);
    match role {
        // Reads each entity by a `.fields` grant narrower than its non-null columns.
        "member" => {
            ab.can(Action::Read, writer::Entity)
                .fields([writer::Column::Id, writer::Column::Name]);
            ab.can(Action::Read, novel::Entity)
                .fields([novel::Column::Id, novel::Column::Title]);
        }
        // Reads no primary key: a cursor would say what the grant withholds.
        "keyless" => {
            ab.can(Action::Read, novel::Entity)
                .fields([novel::Column::Title]);
            ab.can(Action::Read, volume::Entity)
                .fields([volume::Column::Title]);
        }
        // Reads the writers of one circle by a narrow grant, those of another whole.
        "circle" => {
            ab.can(Action::Read, writer::Entity)
                .when(|p| p.eq(writer::Column::Circle, 1))
                .fields([writer::Column::Id]);
            ab.can(Action::Read, writer::Entity)
                .when(|p| p.eq(writer::Column::Circle, 2));
        }
        // Reads other circles only, and the novels of another shelf only, narrowly:
        // the batches here read no `Repo`, so only a mask that runs refuses a row.
        "outsider" => {
            ab.can(Action::Read, writer::Entity)
                .when(|p| p.eq(writer::Column::Circle, 2))
                .fields([writer::Column::Id]);
            ab.can(Action::Read, novel::Entity)
                .when(|p| p.eq(novel::Column::ShelfId, Uuid::nil()))
                .fields([novel::Column::Id]);
        }
        // Reads every non-null column of a writer, its nullable nickname withheld.
        "nickless" => {
            ab.can(Action::Read, writer::Entity).fields([
                writer::Column::Id,
                writer::Column::Name,
                writer::Column::Email,
                writer::Column::Circle,
            ]);
        }
        // A caller a token backs, by a grant only a read could decide.
        "lister" => {
            ab.can(Action::Read, secretive::Entity)
                .when(|p| p.eq(secretive::Column::Listed, true));
        }
        // No grant on either.
        "stranger" => {}
        _ => {
            ab.can(Action::Read, writer::Entity);
            ab.can(Action::Read, novel::Entity);
        }
    }
    ab.build().expect("a valid rule set")
}

/// Attaches a member's ability on the field it guards, whatever the request carries.
#[injectable]
#[derive(Default, Layer)]
struct ReadingAsMember;

#[async_trait]
impl Guard for ReadingAsMember {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(AmbientAbility::new(ability_of("member")));
        Ok(())
    }
}

impl GuardFor<GraphqlView> for ReadingAsMember {}

/// The parent a hand-written field resolver answers under.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct Reading {
    id: i32,
}

/// A wire type's `Result` behind another name.
type Answered<T> = Result<T, ServiceError>;

/// Every shape of an answer holding a wire type's rows that a client selects into.
const WIRE_ANSWERS: [&str; 19] = [
    "lead",
    "bareLead",
    "renamedLead",
    "boxedLead",
    "sharedLead",
    "borrowedLead",
    "lockedLead",
    "readWriteLockedLead",
    "editors",
    "optionalEditors",
    "editorsOrNone",
    "fallibleEditors",
    "dequeEditors",
    "linkedEditors",
    "sortedEditors",
    "hashedEditors",
    "boxedEditors",
    "sharedEditors",
    "arrayEditors",
];

/// Every shape of an answer holding a wire type's rows that serializes whole, a
/// client selecting nothing into it.
const WIRE_SCALARS: [&str; 4] = ["jsonLead", "jsonEditors", "roster", "sortedRoster"];

/// Every shape of a connection a body builds of a wire type's rows.
const HAND_CONNECTIONS: [&str; 3] = ["handNovels", "optionalHandNovels", "boxedHandNovels"];

/// Every connection whose nodes hold a row behind a wrapper, a client selecting into it.
const WRAPPED_NODES: [&str; 3] = ["optionalNodes", "boxedNodes", "sharedNodes"];

/// A connection's own names, so each wrapper's page is a type of its own: a wrapper
/// keeps its row's GraphQL name, and one name serves one Rust type.
macro_rules! page_named {
    ($connection:ident, $edge:ident, $name:literal) => {
        struct $connection;

        impl ConnectionNameType for $connection {
            fn type_name<T: OutputType>() -> String {
                concat!($name, "Connection").to_owned()
            }
        }

        struct $edge;

        impl EdgeNameType for $edge {
            fn type_name<T: OutputType>() -> String {
                concat!($name, "Edge").to_owned()
            }
        }
    };
}

page_named!(OptionalPage, OptionalEdge, "Optional");
page_named!(BoxedPage, BoxedEdge, "Boxed");
page_named!(SharedPage, SharedEdge, "Shared");
page_named!(JsonPage, JsonEdge, "Json");

/// A page whose nodes are the first novel, each as `wrap` holds it.
type WrappedPage<Node, Name, EdgeName> =
    Connection<String, Node, EmptyFields, EmptyFields, Name, EdgeName>;

fn wrapped_page<Node, Name, EdgeName>(
    wrap: impl Fn(novel::Novel) -> Node,
) -> WrappedPage<Node, Name, EdgeName>
where
    Node: OutputType,
    Name: ConnectionNameType,
    EdgeName: EdgeNameType,
{
    let mut page = Connection::new(false, false);
    let first = &novel_rows()[0];
    page.edges.push(Edge::new(
        novel_ids()[0].to_string(),
        wrap(novel::Novel::from(first)),
    ));
    page
}

/// A page of one edge holding `node`.
fn first_edge<Node, Name, EdgeName>(node: Node) -> WrappedPage<Node, Name, EdgeName>
where
    Node: OutputType,
    Name: ConnectionNameType,
    EdgeName: EdgeNameType,
{
    let mut page = Connection::new(false, false);
    page.edges.push(Edge::new("first".to_owned(), node));
    page
}

/// What a body answers holding a wire type's row in a column of its own, served by a
/// `#[field_resolver]` of that object, which masks what it answers.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct Casting {
    role: String,
    #[graphql(skip)]
    star: writer::Writer,
}

/// An object of a body's own, holding no wire type.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct Blurb {
    text: String,
    footnote: Footnote,
}

#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct Footnote {
    text: String,
}

/// The first of two novels, a page behind it, as a body builds it from a loader.
async fn hand_page(novels: &DataLoader<RowsNovels>) -> GqlResult<Connection<String, novel::Novel>> {
    let page = novels
        .load_one(nest_rs_seaorm::__private::relation_key(shelf_id(), 1, None))
        .await?
        .unwrap_or_default();
    Ok(page.into_connection(true))
}

#[resolver]
struct LibraryResolver;

#[operations]
impl LibraryResolver {
    #[query]
    #[authorize(Read, book::Entity)]
    async fn books(&self) -> GqlResult<Vec<book::Book>> {
        Ok(vec![book::Book {
            id: Uuid::nil().to_string(),
            writer_id: writer_id().to_string(),
        }])
    }

    #[query]
    #[authorize(Read, shelf::Entity)]
    async fn shelves(&self) -> GqlResult<Vec<shelf::Shelf>> {
        Ok(vec![shelf::Shelf {
            id: shelf_id().to_string(),
        }])
    }

    /// Three parents, each reaching one writer through its relation.
    #[query]
    #[authorize(Read, book::Entity)]
    async fn three_books(&self) -> GqlResult<Vec<book::Book>> {
        Ok((1..=3)
            .map(|id| book::Book {
                id: Uuid::from_u128(id).to_string(),
                writer_id: writer_id().to_string(),
            })
            .collect())
    }

    /// Three parents, each listing one shelf's novels.
    #[query]
    #[authorize(Read, shelf::Entity)]
    async fn three_shelves(&self) -> GqlResult<Vec<shelf::Shelf>> {
        Ok((0..3)
            .map(|_| shelf::Shelf {
                id: shelf_id().to_string(),
            })
            .collect())
    }

    #[query]
    #[public]
    async fn reading(&self) -> Reading {
        Reading { id: 1 }
    }

    /// Three parents, each answering a row through a hand-written field.
    #[query]
    #[public]
    async fn three_readings(&self) -> Vec<Reading> {
        (1..=3).map(|id| Reading { id }).collect()
    }

    #[field_resolver]
    async fn lead(&self, _parent: &Reading) -> GqlResult<Option<writer::Writer>> {
        Ok(Some(lead_row()))
    }

    #[field_resolver]
    async fn bare_lead(&self, _parent: &Reading) -> writer::Writer {
        lead_row()
    }

    #[field_resolver]
    async fn renamed_lead(&self, _parent: &Reading) -> Answered<writer::Writer> {
        Ok(lead_row())
    }

    #[field_resolver]
    async fn boxed_lead(&self, _parent: &Reading) -> Box<writer::Writer> {
        Box::new(lead_row())
    }

    #[field_resolver]
    async fn shared_lead(&self, _parent: &Reading) -> Arc<writer::Writer> {
        Arc::new(lead_row())
    }

    #[field_resolver]
    async fn borrowed_lead(&self, _parent: &Reading) -> Cow<'static, writer::Writer> {
        Cow::Owned(lead_row())
    }

    #[field_resolver]
    async fn locked_lead(&self, _parent: &Reading) -> tokio::sync::Mutex<writer::Writer> {
        tokio::sync::Mutex::new(lead_row())
    }

    #[field_resolver]
    async fn read_write_locked_lead(
        &self,
        _parent: &Reading,
    ) -> tokio::sync::RwLock<writer::Writer> {
        tokio::sync::RwLock::new(lead_row())
    }

    #[field_resolver]
    async fn editors(&self, _parent: &Reading) -> GqlResult<Vec<writer::Writer>> {
        Ok(vec![lead_row()])
    }

    #[field_resolver]
    async fn optional_editors(&self, _parent: &Reading) -> Option<Vec<writer::Writer>> {
        Some(vec![lead_row()])
    }

    #[field_resolver]
    async fn editors_or_none(&self, _parent: &Reading) -> Vec<Option<writer::Writer>> {
        vec![Some(lead_row()), None]
    }

    #[field_resolver]
    async fn fallible_editors(&self, _parent: &Reading) -> Vec<Result<writer::Writer, GqlError>> {
        vec![Ok(lead_row())]
    }

    #[field_resolver]
    async fn deque_editors(&self, _parent: &Reading) -> VecDeque<writer::Writer> {
        VecDeque::from([lead_row()])
    }

    #[field_resolver]
    async fn linked_editors(&self, _parent: &Reading) -> LinkedList<writer::Writer> {
        LinkedList::from([lead_row()])
    }

    #[field_resolver]
    async fn sorted_editors(&self, _parent: &Reading) -> BTreeSet<writer::Writer> {
        BTreeSet::from([lead_row()])
    }

    #[field_resolver]
    async fn hashed_editors(&self, _parent: &Reading) -> HashSet<writer::Writer> {
        HashSet::from([lead_row()])
    }

    #[field_resolver]
    async fn boxed_editors(&self, _parent: &Reading) -> Box<[writer::Writer]> {
        vec![lead_row()].into_boxed_slice()
    }

    #[field_resolver]
    async fn shared_editors(&self, _parent: &Reading) -> Arc<[writer::Writer]> {
        Arc::from([lead_row()])
    }

    #[field_resolver]
    async fn array_editors(&self, _parent: &Reading) -> [writer::Writer; 1] {
        [lead_row()]
    }

    #[field_resolver]
    async fn json_lead(&self, _parent: &Reading) -> Json<writer::Writer> {
        Json(lead_row())
    }

    #[field_resolver]
    async fn json_editors(&self, _parent: &Reading) -> Json<Vec<writer::Writer>> {
        Json(vec![lead_row()])
    }

    #[field_resolver]
    async fn roster(&self, _parent: &Reading) -> HashMap<String, writer::Writer> {
        HashMap::from([("lead".to_owned(), lead_row())])
    }

    #[field_resolver]
    async fn sorted_roster(&self, _parent: &Reading) -> BTreeMap<String, writer::Writer> {
        BTreeMap::from([("lead".to_owned(), lead_row())])
    }

    #[field_resolver]
    async fn hand_novels(
        &self,
        _parent: &Reading,
        novels: &DataLoader<RowsNovels>,
    ) -> GqlResult<Connection<String, novel::Novel>> {
        hand_page(novels).await
    }

    #[field_resolver]
    async fn optional_hand_novels(
        &self,
        _parent: &Reading,
        novels: &DataLoader<RowsNovels>,
    ) -> GqlResult<Option<Connection<String, novel::Novel>>> {
        hand_page(novels).await.map(Some)
    }

    #[field_resolver]
    async fn boxed_hand_novels(
        &self,
        _parent: &Reading,
        novels: &DataLoader<RowsNovels>,
    ) -> GqlResult<Box<Connection<String, novel::Novel>>> {
        hand_page(novels).await.map(Box::new)
    }

    #[field_resolver]
    async fn optional_nodes(
        &self,
        _parent: &Reading,
    ) -> WrappedPage<Option<novel::Novel>, OptionalPage, OptionalEdge> {
        let mut page: WrappedPage<Option<novel::Novel>, OptionalPage, OptionalEdge> =
            wrapped_page(Some);
        page.edges.push(Edge::new(novel_ids()[1].to_string(), None));
        page
    }

    #[field_resolver]
    async fn boxed_nodes(
        &self,
        _parent: &Reading,
    ) -> WrappedPage<Box<novel::Novel>, BoxedPage, BoxedEdge> {
        wrapped_page(Box::new)
    }

    #[field_resolver]
    async fn shared_nodes(
        &self,
        _parent: &Reading,
    ) -> WrappedPage<Arc<novel::Novel>, SharedPage, SharedEdge> {
        wrapped_page(Arc::new)
    }

    #[field_resolver]
    async fn json_nodes(
        &self,
        _parent: &Reading,
    ) -> WrappedPage<Json<novel::Novel>, JsonPage, JsonEdge> {
        wrapped_page(Json)
    }

    #[field_resolver]
    async fn casting(&self, _parent: &Reading) -> Casting {
        Casting {
            role: "lead".to_owned(),
            star: lead_row(),
        }
    }

    #[field_resolver]
    async fn star(&self, parent: &Casting) -> writer::Writer {
        parent.star.clone()
    }

    #[field_resolver]
    async fn blurb(&self, _parent: &Reading) -> Blurb {
        Blurb {
            text: "a blurb".to_owned(),
            footnote: Footnote {
                text: "a footnote".to_owned(),
            },
        }
    }

    #[field_resolver]
    async fn motto(&self, _parent: &Reading) -> &'static str {
        "a motto"
    }

    #[field_resolver]
    async fn shared_footnote(&self, _parent: &Reading) -> &'static Footnote {
        static SHARED: Footnote = Footnote {
            text: String::new(),
        };
        &SHARED
    }

    #[field_resolver]
    #[use_guards(ReadingAsMember)]
    async fn guarded_lead(&self, _parent: &Reading) -> writer::Writer {
        lead_row()
    }

    #[field_resolver]
    async fn invented_lead(&self, _parent: &Reading) -> writer::Writer {
        writer::Writer {
            id: "anonymous".to_owned(),
            name: "Anonymous".to_owned(),
            email: String::new(),
            nickname: None,
            circle: 0,
        }
    }

    #[field_resolver]
    async fn unlisted(&self, _parent: &Reading) -> secretive::Secretive {
        unlisted_row()
    }

    #[field_resolver]
    async fn maybe_unlisted(&self, _parent: &Reading) -> Option<secretive::Secretive> {
        Some(unlisted_row())
    }

    #[field_resolver]
    async fn unlisted_roll(&self, _parent: &Reading) -> Vec<secretive::Secretive> {
        vec![unlisted_row()]
    }

    #[field_resolver]
    async fn lead_name(&self, _parent: &Reading) -> String {
        writer_row().name
    }
}

#[module(
    imports = [GraphqlModule, SeaOrmDatabaseModule],
    providers = [
        BooksService,
        ShelvesService,
        Rows,
        RoleGuard,
        ReadingAsMember,
        EndpointGuards<GraphqlView, (RoleGuard,)>,
        LibraryResolver,
    ],
)]
struct LibraryModule;

async fn library() -> TestApp {
    crate::over_an_unconnected_pool()
        .module::<LibraryModule>()
        .build()
        .await
        .expect("boots")
}

/// The one refusal an answer carries, read as a client reads it.
#[track_caller]
fn assert_forbidden(answer: &Value, fields: &[&str]) {
    let errors = answer["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("a refusal: {answer}"));
    assert_eq!(errors.len(), 1, "one refusal: {answer}");
    assert_eq!(errors[0]["extensions"]["code"], "FORBIDDEN", "{answer}");
    assert_eq!(errors[0]["extensions"]["fields"], json!(fields), "{answer}");
}

#[tokio::test]
async fn a_has_one_serves_a_selection_its_narrow_grant_allows() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(&app, "member", "{ books { writer { id name } } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["books"][0]["writer"],
        json!({ "id": writer_id().to_string(), "name": "Ada" }),
    );
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_has_one_refuses_a_withheld_selection_naming_the_field_with_no_error_line() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(&app, "member", "{ books { writer { id email } } }").await;

    assert_forbidden(&read, &["email"]);
    assert_eq!(read["errors"][0]["path"], json!(["books", 0, "writer"]));
    assert_eq!(read["data"]["books"][0]["writer"], Value::Null, "{read}");
    assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
    assert_no_error_line(&logs);
    let denied: Vec<_> = logs
        .events_at(tracing::Level::WARN)
        .into_iter()
        .filter(|event| event.field("reason").as_deref() == Some("field_not_granted"))
        .collect();
    assert_eq!(denied.len(), 1, "one line for one refusal: {denied:?}");
    assert!(!denied[0].quotes(SECRET_EMAIL));
    // A relation `#[expose]` emits has no host of its own.
    assert_eq!(denied[0].field("transport").as_deref(), Some("graphql"));
    assert_eq!(denied[0].field("kind").as_deref(), Some("query"));
    assert_eq!(denied[0].field("field").as_deref(), Some("writer"));
    assert_eq!(denied[0].field("host"), None);
}

#[tokio::test]
async fn a_has_one_answers_a_caller_no_grant_narrows_as_before() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(&app, "reader", "{ books { writer { id name email } } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["books"][0]["writer"]["email"], SECRET_EMAIL);
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn two_aliases_of_a_has_one_answer_each_its_own_selection() {
    let logs = LogCapture::install();
    let app = library().await;

    for query in [
        "{ books { granted: writer { id name } withheld: writer { email } } }",
        "{ books { withheld: writer { email } granted: writer { id name } } }",
    ] {
        let read = answer(&app, "member", query).await;

        assert_forbidden(&read, &["email"]);
        let book = &read["data"]["books"][0];
        assert_eq!(book["granted"]["name"], "Ada", "{read}");
        assert_eq!(book["withheld"], Value::Null, "{read}");
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
    }
    assert_no_error_line(&logs);
}

/// Two selections of one response key are merged by the response, not the
/// executor, so each is a field of its own.
#[tokio::test]
async fn a_withheld_field_merged_into_a_granted_selection_of_one_key_is_refused() {
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ books { writer { id } writer { email } } }",
    )
    .await;

    assert_forbidden(&read, &["email"]);
    assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
}

#[tokio::test]
async fn a_fragment_selects_what_it_spreads() {
    let app = library().await;

    for query in [
        "{ books { writer { ... on Writer { email } } } }",
        "{ books { writer { ...Contact } } } fragment Contact on Writer { email }",
        "{ books { writer { mail: email } } }",
    ] {
        let read = answer(&app, "member", query).await;

        assert_forbidden(&read, &["email"]);
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
    }
}

/// A root's rule: what the wire type can read as `null` reads so, refusing
/// nothing, and the row's column never reaches the client.
#[tokio::test]
async fn a_withheld_nullable_column_reads_null_where_the_row_still_fits() {
    let app = library().await;

    let read = answer(&app, "nickless", "{ books { writer { id nickname } } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["books"][0]["writer"],
        json!({ "id": writer_id().to_string(), "nickname": Value::Null }),
    );
}

/// A root's rule: where the row no longer fits its type, every selected column
/// the grant withholds is refused, a nullable one among them.
#[tokio::test]
async fn a_withheld_nullable_column_is_refused_where_the_row_no_longer_fits() {
    let app = library().await;

    let read = answer(&app, "circle", "{ books { writer { id nickname } } }").await;

    assert_forbidden(&read, &["nickname"]);
    assert!(!read.to_string().contains("countess"), "{read}");
}

#[tokio::test]
async fn a_row_is_judged_on_what_it_holds() {
    let app = library().await;

    let read = answer(&app, "circle", "{ books { writer { id } } }").await;
    assert_eq!(
        read["data"]["books"][0]["writer"]["id"],
        writer_id().to_string(),
        "{read}"
    );

    let read = answer(&app, "circle", "{ books { writer { name } } }").await;
    assert_forbidden(&read, &["name"]);
}

#[tokio::test]
async fn a_row_the_ability_refuses_reads_null() {
    let logs = LogCapture::install();
    let app = library().await;

    for role in ["outsider", "stranger"] {
        let read = answer(&app, role, "{ books { writer { id email } } }").await;

        assert!(read.get("errors").is_none(), "{role}: {read}");
        assert_eq!(read["data"]["books"][0]["writer"], Value::Null, "{role}");
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_has_many_serves_a_selection_its_narrow_grant_allows() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ shelves { novels { edges { cursor node { id title } } nodes { title } } } }",
    )
    .await;

    assert!(read.get("errors").is_none(), "{read}");
    let novels = &read["data"]["shelves"][0]["novels"];
    assert_eq!(
        novels["edges"][1]["node"],
        json!({ "id": novel_ids()[1].to_string(), "title": "Sketches" }),
    );
    assert_eq!(novels["edges"][1]["cursor"], novel_ids()[1].to_string());
    assert_eq!(novels["nodes"][0]["title"], "Sketches");
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_has_many_refuses_a_withheld_selection_naming_the_field_with_no_error_line() {
    let logs = LogCapture::install();
    let app = library().await;

    for query in [
        "{ shelves { novels { edges { node { title isbn } } } } }",
        "{ shelves { novels { nodes { isbn } } } }",
        "{ shelves { novels { ...Page } } } fragment Page on NovelConnection { edges { node { ... on Novel { isbn } } } }",
    ] {
        let read = answer(&app, "member", query).await;

        assert_forbidden(&read, &["isbn"]);
        assert!(!read.to_string().contains(SECRET_ISBN), "{read}");
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_has_many_answers_a_caller_no_grant_narrows_as_before() {
    let app = library().await;

    let read = answer(
        &app,
        "reader",
        "{ shelves { novels { edges { node { id isbn } } } } }",
    )
    .await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["shelves"][0]["novels"]["edges"][0]["node"]["isbn"],
        SECRET_ISBN
    );
}

#[tokio::test]
async fn two_aliases_of_a_has_many_answer_each_its_own_selection() {
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ shelves { granted: novels(first: 1) { nodes { title } } withheld: novels(first: 1) { nodes { isbn } } } }",
    )
    .await;

    assert_forbidden(&read, &["isbn"]);
    assert!(!read.to_string().contains(SECRET_ISBN), "{read}");
}

#[tokio::test]
async fn a_has_many_pages_under_a_narrow_grant() {
    let app = library().await;

    let first = answer(
        &app,
        "member",
        "{ shelves { novels(first: 1) { edges { node { title } } pageInfo { hasNextPage endCursor } } } }",
    )
    .await;
    assert!(first.get("errors").is_none(), "{first}");
    let page = &first["data"]["shelves"][0]["novels"]["pageInfo"];
    assert_eq!(page["hasNextPage"], true, "{first}");
    let cursor = page["endCursor"].as_str().expect("a cursor");

    let second = answer(
        &app,
        "member",
        &format!(
            r#"{{ shelves {{ novels(first: 1, after: "{cursor}") {{ nodes {{ id }} pageInfo {{ hasNextPage hasPreviousPage }} }} }} }}"#
        ),
    )
    .await;
    assert!(second.get("errors").is_none(), "{second}");
    let page = &second["data"]["shelves"][0]["novels"];
    assert_eq!(page["nodes"][0]["id"], novel_ids()[1].to_string());
    assert_eq!(page["pageInfo"]["hasNextPage"], false, "{second}");
    assert_eq!(page["pageInfo"]["hasPreviousPage"], true, "{second}");
}

/// A client matches a refusal to its schema, under a page's nodes and its edges, and
/// where only a cursor reads the column: no selection names it there, and the schema's
/// is the one name the client knows it by.
#[tokio::test]
async fn a_refusal_names_a_two_word_field_as_the_schema_spells_it() {
    let app = library().await;

    for query in [
        "{ shelves { novels { nodes { shelfId } } } }",
        "{ shelves { novels { edges { node { shelfId } } } } }",
    ] {
        assert_forbidden(&answer(&app, "keyless", query).await, &["shelfId"]);
    }
    for query in [
        "{ shelves { volumes { edges { cursor } } } }",
        "{ shelves { volumes { pageInfo { endCursor } } } }",
    ] {
        assert_forbidden(&answer(&app, "keyless", query).await, &["volumeId"]);
    }
}

/// A cursor is the row's primary key, so a grant withholding the key withholds it.
#[tokio::test]
async fn a_cursor_is_refused_where_the_grant_withholds_the_primary_key() {
    let app = library().await;

    for query in [
        "{ shelves { novels { edges { cursor } } } }",
        "{ shelves { novels { pageInfo { endCursor } } } }",
        "{ shelves { novels { pageInfo { startCursor } nodes { title } } } }",
    ] {
        let read = answer(&app, "keyless", query).await;

        assert_forbidden(&read, &["id"]);
        assert!(
            !read.to_string().contains(&novel_ids()[0].to_string()),
            "{read}"
        );
    }

    let read = answer(
        &app,
        "keyless",
        "{ shelves { novels { nodes { title } pageInfo { hasNextPage } } } }",
    )
    .await;
    assert!(read.get("errors").is_none(), "{read}");
}

#[tokio::test]
async fn a_row_the_ability_refuses_leaves_a_has_many_with_its_edge() {
    let logs = LogCapture::install();
    let app = library().await;

    for role in ["outsider", "stranger"] {
        let read = answer(
            &app,
            role,
            "{ shelves { novels { edges { cursor node { id isbn } } } } }",
        )
        .await;

        assert!(read.get("errors").is_none(), "{role}: {read}");
        assert_eq!(
            read["data"]["shelves"][0]["novels"]["edges"],
            json!([]),
            "{role}"
        );
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_field_resolver_answering_a_wire_type_serves_what_the_grant_allows() {
    let logs = LogCapture::install();
    let app = library().await;

    for field in WIRE_ANSWERS {
        let read = answer(
            &app,
            "member",
            &format!("{{ reading {{ {field} {{ id name }} }} }}"),
        )
        .await;

        assert!(read.get("errors").is_none(), "{field}: {read}");
        assert!(read.to_string().contains("Ada"), "{field}: {read}");
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_field_resolver_answering_a_wire_type_refuses_what_the_grant_withholds() {
    let logs = LogCapture::install();
    let app = library().await;

    for field in WIRE_ANSWERS {
        let read = answer(
            &app,
            "member",
            &format!("{{ reading {{ {field} {{ id email }} }} }}"),
        )
        .await;

        assert_forbidden(&read, &["email"]);
        assert!(!read.to_string().contains(SECRET_EMAIL), "{field}: {read}");
    }
    assert_no_error_line(&logs);
    let denied: Vec<_> = logs
        .events_at(tracing::Level::WARN)
        .into_iter()
        .filter(|event| event.field("reason").as_deref() == Some("field_not_granted"))
        .collect();
    assert_eq!(denied.len(), WIRE_ANSWERS.len(), "{denied:?}");
    for (line, field) in denied.iter().zip(WIRE_ANSWERS) {
        assert_eq!(line.field("host").as_deref(), Some("LibraryResolver"));
        assert!(line.field("method").is_some(), "{:?}", line.fields);
        assert_eq!(line.field("kind").as_deref(), Some("query"));
        assert_eq!(line.field("field").as_deref(), Some(field));
    }
}

#[tokio::test]
async fn a_field_resolver_answers_a_caller_no_grant_narrows_as_before() {
    let app = library().await;

    for field in WIRE_ANSWERS {
        let read = answer(
            &app,
            "reader",
            &format!("{{ reading {{ {field} {{ email }} }} }}"),
        )
        .await;

        assert!(read.get("errors").is_none(), "{field}: {read}");
        assert!(read.to_string().contains(SECRET_EMAIL), "{field}: {read}");
    }
}

/// A `None` in a list is no row: it reads as it stands, beside the rows the mask cut.
#[tokio::test]
async fn a_field_resolver_keeps_the_none_of_a_list() {
    let app = library().await;

    let read = answer(&app, "member", "{ reading { editorsOrNone { name } } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["reading"]["editorsOrNone"],
        json!([{ "name": "Ada" }, Value::Null]),
    );
}

/// A scalar serializes whole, so what the mask took refuses it wherever it stands.
#[tokio::test]
async fn a_field_resolver_answering_a_scalar_of_a_wire_type_refuses_every_column_cut() {
    let logs = LogCapture::install();
    let app = library().await;

    for field in WIRE_SCALARS {
        let read = answer(&app, "member", &format!("{{ reading {{ {field} }} }}")).await;

        assert_forbidden(&read, &["circle", "email", "nickname"]);
        assert!(!read.to_string().contains(SECRET_EMAIL), "{field}: {read}");
        assert!(!read.to_string().contains("countess"), "{field}: {read}");
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_field_resolver_answers_a_scalar_of_a_wire_type_to_a_caller_no_grant_narrows() {
    let app = library().await;

    for field in WIRE_SCALARS {
        let read = answer(&app, "reader", &format!("{{ reading {{ {field} }} }}")).await;

        assert!(read.get("errors").is_none(), "{field}: {read}");
        assert!(read.to_string().contains(SECRET_EMAIL), "{field}: {read}");
    }
}

/// What the type can read as `null` reads so, a scalar's as an object's.
#[tokio::test]
async fn a_scalar_still_fitting_its_type_ships_cut() {
    let app = library().await;

    let read = answer(&app, "nickless", "{ reading { jsonLead } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["reading"]["jsonLead"]["name"], "Ada");
    assert_eq!(read["data"]["reading"]["jsonLead"]["nickname"], Value::Null);
}

#[tokio::test]
async fn a_hand_built_connection_serves_what_the_grant_allows_and_keeps_its_flags() {
    let logs = LogCapture::install();
    let app = library().await;

    for field in HAND_CONNECTIONS {
        let read = answer(
            &app,
            "member",
            &format!(
                "{{ reading {{ {field} {{ edges {{ cursor node {{ id title }} }} nodes {{ title }} pageInfo {{ hasNextPage hasPreviousPage endCursor }} }} }} }}"
            ),
        )
        .await;

        assert!(read.get("errors").is_none(), "{field}: {read}");
        let page = &read["data"]["reading"][field];
        let first = novel_ids()[0].to_string();
        assert_eq!(
            page["edges"],
            json!([{ "cursor": first, "node": { "id": first, "title": "Sketches" } }]),
            "{field}",
        );
        assert_eq!(page["nodes"], json!([{ "title": "Sketches" }]), "{field}");
        assert_eq!(
            page["pageInfo"],
            json!({ "hasNextPage": true, "hasPreviousPage": true, "endCursor": first }),
            "{field}",
        );
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_hand_built_connection_refuses_what_the_grant_withholds() {
    let logs = LogCapture::install();
    let app = library().await;

    for field in HAND_CONNECTIONS {
        for selection in ["nodes { id isbn }", "edges { node { title isbn } }"] {
            let read = answer(
                &app,
                "member",
                &format!("{{ reading {{ {field} {{ {selection} }} }} }}"),
            )
            .await;

            assert_forbidden(&read, &["isbn"]);
            assert!(!read.to_string().contains(SECRET_ISBN), "{field}: {read}");
        }
    }
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_hand_built_connection_answers_a_caller_no_grant_narrows() {
    let app = library().await;

    for field in HAND_CONNECTIONS {
        let read = answer(
            &app,
            "reader",
            &format!("{{ reading {{ {field} {{ nodes {{ isbn }} }} }} }}"),
        )
        .await;

        assert!(read.get("errors").is_none(), "{field}: {read}");
        assert!(read.to_string().contains(SECRET_ISBN), "{field}: {read}");
    }
}

/// A cursor spells the row's key on a loader's page, so a selected one reads as that
/// column, whatever built the connection.
#[tokio::test]
async fn a_hand_built_connections_cursor_is_refused_under_a_withheld_key() {
    let app = library().await;

    for selection in ["edges { cursor }", "pageInfo { endCursor }"] {
        let read = answer(
            &app,
            "keyless",
            &format!("{{ reading {{ handNovels {{ {selection} }} }} }}"),
        )
        .await;

        assert_forbidden(&read, &["id"]);
        assert!(
            !read.to_string().contains(&novel_ids()[0].to_string()),
            "{read}"
        );
    }

    let read = answer(
        &app,
        "keyless",
        "{ reading { handNovels { nodes { title } pageInfo { hasNextPage } } } }",
    )
    .await;
    assert!(read.get("errors").is_none(), "{read}");
}

#[tokio::test]
async fn a_row_the_ability_refuses_leaves_a_hand_built_connection_with_its_edge() {
    let app = library().await;

    // A visitor and a caller a token backs, each reading no novel of this shelf.
    for role in ["outsider", "stranger", ""] {
        let read = answer(
            &app,
            role,
            "{ reading { handNovels { edges { cursor node { id isbn } } pageInfo { hasNextPage } } } }",
        )
        .await;

        assert!(read.get("errors").is_none(), "{role}: {read}");
        let page = &read["data"]["reading"]["handNovels"];
        assert_eq!(page["edges"], json!([]), "{role}");
        assert_eq!(page["pageInfo"]["hasNextPage"], true, "{role}");
    }
}

/// A row the body built is no column a grant can read back: answered opaquely, the
/// failure the operator's, as a root's.
#[tokio::test]
async fn a_value_the_mask_cannot_read_back_answers_internal() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(&app, "member", "{ reading { inventedLead { id name } } }").await;

    let error = &read["errors"][0];
    assert_eq!(error["extensions"]["code"], "INTERNAL", "{read}");
    assert_eq!(error["message"], OPAQUE_CLIENT_MESSAGE, "{read}");
    assert!(!read.to_string().contains("Anonymous"), "{read}");
    let event = logs.expect_one("nest_rs::authz", "response masking failed");
    assert_eq!(event.field("kind").as_deref(), Some("query"));
    assert_eq!(event.field("field").as_deref(), Some("inventedLead"));
    assert_eq!(event.field("host").as_deref(), Some("LibraryResolver"));
    assert_eq!(event.field("method").as_deref(), Some("invented_lead"));
    assert_no_error_line(&logs);
}

/// A visitor's reads are withheld below its root, so no read applied a rule only a
/// read could decide: the grant grants nothing, as on a pushed row.
#[tokio::test]
async fn a_visitors_answer_is_judged_as_no_read_filtered_it() {
    let app = library().await;

    let read = answer(&app, "", "{ reading { unlisted { id name } } }").await;
    assert_forbidden(&read, &["id", "name"]);
    assert!(!read.to_string().contains(UNLISTED_NAME), "{read}");

    let read = answer(&app, "", "{ reading { maybeUnlisted { id name } } }").await;
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["reading"]["maybeUnlisted"], Value::Null);

    let read = answer(&app, "", "{ reading { unlistedRoll { id name } } }").await;
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["reading"]["unlistedRoll"], json!([]));
}

/// Its fields are all granted, so only the row's refusal can withhold them: a type
/// that cannot read `null` ships none of them.
#[tokio::test]
async fn a_visitors_answer_a_denial_refuses_ships_nothing() {
    let app = library().await;

    let read = answer(&app, "wary-visitor", "{ reading { unlisted { id name } } }").await;

    assert_forbidden(&read, &["id", "name"]);
    assert!(!read.to_string().contains(UNLISTED_NAME), "{read}");
}

/// `mask-provenance`'s stated limit: a caller a token backs reads through `Repo`, so
/// its answer keeps the trust a read earns, whatever read it.
#[tokio::test]
async fn an_answer_to_a_caller_a_token_backs_keeps_the_trust_a_read_earns() {
    let app = library().await;

    let read = answer(&app, "lister", "{ reading { unlisted { name } } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["reading"]["unlisted"]["name"], UNLISTED_NAME);
}

/// No grant reads the wire type, and no row is dropped from a lone answer: each
/// selected column is what the grant withholds.
#[tokio::test]
async fn a_field_resolver_refuses_a_caller_no_grant_reads() {
    let app = library().await;

    let read = answer(&app, "stranger", "{ reading { lead { email } } }").await;

    assert_forbidden(&read, &["email"]);
    assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
}

#[tokio::test]
async fn a_field_resolver_answering_no_wire_type_runs_no_mask() {
    let app = library().await;

    let read = answer(&app, "stranger", "{ reading { leadName } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["reading"]["leadName"], "Ada");
}

/// A visitor's field grants hold off its root too, a bare `#[public]`'s included.
#[tokio::test]
async fn a_field_resolver_masks_a_visitors_answer_by_its_grant() {
    let app = library().await;

    let read = answer(&app, "", "{ reading { lead { name } } }").await;
    assert_eq!(read["data"]["reading"]["lead"]["name"], "Ada", "{read}");

    let read = answer(&app, "", "{ reading { lead { email } } }").await;
    assert_forbidden(&read, &["email"]);
    assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
}

/// The parent a field resolver answers under in an app with no ability guard.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct UngrantedReading {
    id: i32,
}

/// Gates nothing, so an app attaching no ability boots it.
#[resolver]
struct UngrantedLibraryResolver;

#[operations]
impl UngrantedLibraryResolver {
    #[query]
    #[public]
    async fn ungranted_reading(&self) -> UngrantedReading {
        UngrantedReading { id: 1 }
    }

    /// Three parents, each answering a row through a hand-written field.
    #[query]
    #[public]
    async fn three_ungranted_readings(&self) -> Vec<UngrantedReading> {
        (1..=3).map(|id| UngrantedReading { id }).collect()
    }

    /// Three parents, each reaching one writer through its relation.
    #[query]
    #[public]
    async fn three_ungranted_books(&self) -> Vec<book::Book> {
        (1..=3)
            .map(|id| book::Book {
                id: Uuid::from_u128(id).to_string(),
                writer_id: writer_id().to_string(),
            })
            .collect()
    }

    #[field_resolver]
    async fn lead(&self, _parent: &UngrantedReading) -> GqlResult<Option<writer::Writer>> {
        Ok(Some(lead_row()))
    }

    #[field_resolver]
    async fn hand_novels(
        &self,
        _parent: &UngrantedReading,
        novels: &DataLoader<RowsNovels>,
    ) -> GqlResult<Connection<String, novel::Novel>> {
        hand_page(novels).await
    }
}

#[module(
    imports = [GraphqlModule, SeaOrmDatabaseModule],
    providers = [BooksService, ShelvesService, Rows, UngrantedLibraryResolver],
)]
struct UngrantedLibraryModule;

/// No ability is attached, so nothing decided what this caller reads: the answer
/// fails closed as a root's and a relation's, an operator's line naming the wiring.
#[tokio::test]
async fn a_field_resolver_refuses_where_no_ability_is_attached() {
    let logs = LogCapture::install();
    let app = crate::over_an_unconnected_pool()
        .module::<UngrantedLibraryModule>()
        .build()
        .await
        .expect("boots");

    for selection in ["lead { email }", "handNovels { nodes { isbn } }"] {
        let read = answer(
            &app,
            "",
            &format!("{{ ungrantedReading {{ {selection} }} }}"),
        )
        .await;

        let error = &read["errors"][0];
        assert_eq!(error["extensions"]["code"], "INTERNAL", "{read}");
        assert_eq!(error["message"], OPAQUE_CLIENT_MESSAGE, "{read}");
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
        assert!(!read.to_string().contains(SECRET_ISBN), "{read}");
    }
    let lines = logs.find("nest_rs::authz", "authorization denied");
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(
        lines[0].field("reason").as_deref(),
        Some("no_ambient_ability")
    );
}

/// No ability under any of three parents is one wiring defect, its line one per
/// operation, whichever field meets it first; each parent keeps its error.
#[tokio::test]
async fn no_ability_under_every_parent_files_one_line() {
    for query in [
        "{ threeUngrantedReadings { lead { email } } }",
        "{ threeUngrantedReadings { handNovels { nodes { isbn } } } }",
        "{ threeUngrantedBooks { writer { email } } }",
    ] {
        let logs = LogCapture::install();
        let app = crate::over_an_unconnected_pool()
            .module::<UngrantedLibraryModule>()
            .build()
            .await
            .expect("boots");

        let read = answer(&app, "", query).await;

        assert_eq!(read["errors"].as_array().map(Vec::len), Some(3), "{read}");
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
        let event = logs.expect_one("nest_rs::authz", "authorization denied");
        assert_eq!(event.level, "error", "{query}");
        assert_eq!(event.field("reason").as_deref(), Some("no_ambient_ability"));
    }
}

/// A row no mask can read back, under every parent, is one fact too.
#[tokio::test]
async fn a_value_the_mask_cannot_read_back_under_every_parent_files_one_line() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ threeReadings { inventedLead { id name } } }",
    )
    .await;

    assert_eq!(read["errors"].as_array().map(Vec::len), Some(3), "{read}");
    logs.expect_one("nest_rs::authz", "response masking failed");
}

/// A guard on the field holds a holder per parent, so the line is the operation's to
/// count, never the holder's.
#[tokio::test]
async fn a_selection_a_field_guard_refuses_the_ability_of_files_one_line() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(&app, "", "{ threeReadings { guardedLead { email } } }").await;

    assert_eq!(read["errors"].as_array().map(Vec::len), Some(3), "{read}");
    let event = logs.expect_one("nest_rs::authz", "authorization denied");
    assert_eq!(event.field("reason").as_deref(), Some("field_not_granted"));
}

/// One refused selection is one fact, however many parents repeat it: a line per
/// operation, an error per path, each carrying its own.
#[tokio::test]
async fn one_refused_selection_files_one_warn_and_one_error_per_parent() {
    for (query, root, path) in [
        (
            "{ threeBooks { writer { email } } }",
            "threeBooks",
            "writer",
        ),
        (
            "{ threeShelves { novels { nodes { isbn } } } }",
            "threeShelves",
            "novels",
        ),
        (
            "{ threeReadings { lead { email } } }",
            "threeReadings",
            "lead",
        ),
    ] {
        let logs = LogCapture::install();
        let app = library().await;

        let read = answer(&app, "member", query).await;

        let errors = read["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("{read}"));
        assert_eq!(errors.len(), 3, "an error per parent: {read}");
        for (at, error) in errors.iter().enumerate() {
            assert_eq!(error["extensions"]["code"], "FORBIDDEN", "{read}");
            assert_eq!(error["path"], json!([root, at, path]), "{read}");
        }
        let event = logs.expect_one("nest_rs::authz", "authorization denied");
        assert_eq!(event.field("reason").as_deref(), Some("field_not_granted"));
    }
}

#[tokio::test]
async fn each_refused_selection_of_an_operation_files_its_own_line() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ threeBooks { writer { email } } again: threeBooks { writer { circle } } }",
    )
    .await;

    assert_eq!(read["errors"].as_array().map(Vec::len), Some(6), "{read}");
    let lines = logs.find("nest_rs::authz", "authorization denied");
    let mut fields: Vec<_> = lines
        .iter()
        .filter_map(|line| line.field("fields"))
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, ["circle", "email"], "{lines:#?}");
}

#[tokio::test]
async fn the_next_operation_files_its_own_line() {
    let logs = LogCapture::install();
    let app = library().await;

    for _ in 0..2 {
        answer(&app, "member", "{ threeBooks { writer { email } } }").await;
    }

    let lines = logs.find("nest_rs::authz", "authorization denied");
    assert_eq!(lines.len(), 2, "{lines:#?}");
}

#[tokio::test]
async fn a_connection_whose_nodes_hold_a_row_masks_it() {
    let logs = LogCapture::install();
    let app = library().await;

    for field in WRAPPED_NODES {
        let read = answer(
            &app,
            "member",
            &format!("{{ reading {{ {field} {{ nodes {{ id title }} edges {{ cursor }} }} }} }}"),
        )
        .await;
        assert!(read.get("errors").is_none(), "{field}: {read}");
        assert_eq!(
            read["data"]["reading"][field]["nodes"][0]["title"], "Sketches",
            "{field}: {read}",
        );

        for selection in ["nodes { id isbn }", "edges { node { title isbn } }"] {
            let read = answer(
                &app,
                "member",
                &format!("{{ reading {{ {field} {{ {selection} }} }} }}"),
            )
            .await;
            assert_forbidden(&read, &["isbn"]);
            assert!(!read.to_string().contains(SECRET_ISBN), "{field}: {read}");
        }
    }
    assert_no_error_line(&logs);
}

/// A `None` node holds no row: it stays beside the rows the mask cut.
#[tokio::test]
async fn a_none_node_stays_in_its_connection() {
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ reading { optionalNodes { edges { node { title } } } } }",
    )
    .await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["reading"]["optionalNodes"]["edges"],
        json!([{ "node": { "title": "Sketches" } }, { "node": Value::Null }]),
    );
}

#[tokio::test]
async fn a_row_the_ability_refuses_leaves_a_wrapped_connection_with_its_edge() {
    let app = library().await;

    for field in WRAPPED_NODES {
        let read = answer(
            &app,
            "outsider",
            &format!("{{ reading {{ {field} {{ edges {{ cursor node {{ id }} }} }} }} }}"),
        )
        .await;

        assert!(read.get("errors").is_none(), "{field}: {read}");
        let kept = read["data"]["reading"][field]["edges"]
            .as_array()
            .map(Vec::len);
        let none_node = usize::from(field == "optionalNodes");
        assert_eq!(kept, Some(none_node), "{field}: {read}");
    }
}

/// A scalar node serializes whole, so what the mask took refuses it wherever it stands.
#[tokio::test]
async fn a_connection_of_a_scalar_of_a_wire_type_refuses_every_column_cut() {
    let logs = LogCapture::install();
    let app = library().await;

    for selection in ["nodes", "edges { node }"] {
        let read = answer(
            &app,
            "member",
            &format!("{{ reading {{ jsonNodes {{ {selection} }} }} }}"),
        )
        .await;

        assert_forbidden(&read, &["isbn", "shelf_id"]);
        assert!(!read.to_string().contains(SECRET_ISBN), "{read}");
    }
    assert_no_error_line(&logs);

    let read = answer(&app, "reader", "{ reading { jsonNodes { nodes } } }").await;
    assert!(read.get("errors").is_none(), "{read}");
    assert!(read.to_string().contains(SECRET_ISBN), "{read}");
}

/// The remedy a refusal below names: the object keeps its column and a field resolver
/// of that object serves it, masked.
#[tokio::test]
async fn an_object_serving_its_row_from_a_field_resolver_masks_it() {
    let logs = LogCapture::install();
    let app = library().await;

    let read = answer(
        &app,
        "member",
        "{ reading { casting { role star { name } } } }",
    )
    .await;
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["reading"]["casting"],
        json!({ "role": "lead", "star": { "name": "Ada" } }),
    );

    let read = answer(&app, "member", "{ reading { casting { star { email } } } }").await;
    assert_forbidden(&read, &["email"]);
    assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
    assert_no_error_line(&logs);
}

/// What refuses a `&'static` row leaves every other `&'static` answer as it is.
#[tokio::test]
async fn a_static_borrow_holding_no_wire_type_runs_no_mask() {
    let app = library().await;

    let read = answer(
        &app,
        "stranger",
        "{ reading { motto sharedFootnote { text } } }",
    )
    .await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["reading"]["motto"], "a motto");
}

#[tokio::test]
async fn an_object_holding_no_wire_type_runs_no_mask() {
    let app = library().await;

    let read = answer(
        &app,
        "stranger",
        "{ reading { blurb { text footnote { text } } } }",
    )
    .await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["reading"]["blurb"]["footnote"]["text"],
        "a footnote"
    );
}

/// An object of a body's own holding the wire type's row in a column.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct Holder {
    role: String,
    lead: writer::Writer,
}

fn holder() -> Holder {
    Holder {
        role: "lead".to_owned(),
        lead: lead_row(),
    }
}

fn first_edge_of_writers() -> Connection<String, writer::Writer> {
    let mut page = Connection::new(false, false);
    page.edges.push(Edge::new("first".to_owned(), lead_row()));
    page
}

/// An object holding a [`Holder`], which holds the row one level down.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct OuterHolder {
    inner: Holder,
}

/// An object taking the columns of another, one of which holds the row.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct FlatOfHolder {
    #[graphql(flatten)]
    holder: Holder,
}

/// An object taking the row's columns as its own.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct FlatHolder {
    role: String,
    #[graphql(flatten)]
    lead: writer::Writer,
}

/// An object merging the row's columns with another's.
#[derive(MergedObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct MergedHolder(writer::Writer, Footnote);

/// A union one of whose members is the wire type.
#[derive(Union)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
enum HeldUnion {
    Writer(writer::Writer),
    Footnote(Footnote),
}

/// An interface one of whose implementors is the wire type.
#[derive(Interface)]
#[graphql(
    crate = "::nest_rs_graphql::async_graphql",
    field(name = "email", ty = "&String")
)]
enum HeldInterface {
    Writer(writer::Writer),
}

/// An object writing the method that answers the row by hand.
struct WrittenHolder;

#[Object(crate = "::nest_rs_graphql::async_graphql")]
impl WrittenHolder {
    async fn lead(&self) -> writer::Writer {
        lead_row()
    }
}

/// One app whose one field resolver answers the shape under test, beside what else
/// its resolver serves.
macro_rules! answering {
    ($app:ident, [$($answer:tt)*], $value:expr $(, { $($beside:tt)* })?) => {
        mod $app {
            use super::*;

            #[derive(Clone, SimpleObject)]
            #[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
            pub(super) struct Parent {
                id: i32,
            }

            #[resolver]
            pub(super) struct AnsweringResolver;

            #[operations]
            impl AnsweringResolver {
                #[query]
                #[public]
                async fn parent(&self) -> Parent {
                    Parent { id: 1 }
                }

                #[field_resolver]
                async fn answer(&self, _parent: &Parent) -> $($answer)* {
                    $value
                }

                $($($beside)*)?
            }

            #[module(
                imports = [GraphqlModule, SeaOrmDatabaseModule],
                providers = [RoleGuard, EndpointGuards<GraphqlView, (RoleGuard,)>, AnsweringResolver],
            )]
            pub(super) struct AnsweringModule;
        }
    };
}

answering!(holding, [Holder], holder());
answering!(holding_in_a_list, [Vec<Holder>], vec![holder()]);
answering!(
    holding_deeper,
    [OuterHolder],
    OuterHolder { inner: holder() }
);
answering!(
    holding_flat,
    [FlatHolder],
    FlatHolder {
        role: "lead".to_owned(),
        lead: lead_row(),
    }
);
answering!(
    holding_merged,
    [MergedHolder],
    MergedHolder(
        lead_row(),
        Footnote {
            text: "a footnote".to_owned()
        }
    )
);
answering!(
    holding_in_a_union,
    [HeldUnion],
    HeldUnion::Writer(lead_row())
);
answering!(
    holding_in_an_interface,
    [HeldInterface],
    HeldInterface::Writer(lead_row())
);
answering!(holding_by_hand, [WrittenHolder], WrittenHolder);
answering!(
    an_edge,
    [Edge<String, writer::Writer, EmptyFields>],
    Edge::new("first".to_owned(), lead_row())
);
answering!(
    a_shared_connection,
    [Arc<Connection<String, writer::Writer>>],
    Arc::new(first_edge(lead_row()))
);
answering!(
    a_connection_of_holders,
    [Connection<String, Holder>],
    first_edge(holder())
);
answering!(
    a_connection_of_flattened_holders,
    [Connection<String, FlatOfHolder>],
    {
        let mut page = Connection::new(false, false);
        page.edges.push(Edge::new(
            "first".to_owned(),
            FlatOfHolder { holder: holder() },
        ));
        page
    }
);
answering!(
    a_connection_holding_a_row_beside_its_edges,
    [Connection<String, writer::Writer, Holder>],
    {
        let mut page = Connection::with_additional_fields(false, false, holder());
        page.edges.push(Edge::new("first".to_owned(), lead_row()));
        page
    }
);
answering!(
    an_edge_holding_a_row_beside_its_node,
    [Connection<String, writer::Writer, EmptyFields, Holder>],
    {
        let mut page = Connection::new(false, false);
        page.edges.push(Edge::with_additional_fields(
            "first".to_owned(),
            lead_row(),
            holder(),
        ));
        page
    }
);
answering!(
    a_page_of_a_list_holding_a_row_beside_its_edges,
    [Vec<Connection<String, writer::Writer, Holder>>],
    vec![Connection::with_additional_fields(false, false, holder())]
);
answering!(
    a_connection_holding_nothing_beside_its_edges,
    [Connection<String, writer::Writer>],
    first_edge_of_writers()
);

// One shape's page in each app, so they share one name.
page_named!(NodesPage, NodesEdge, "Nodes");

/// A page holding `Node`s.
type NodesOf<Node> = WrappedPage<Node, NodesPage, NodesEdge>;

answering!(
    nodes_of_a_vec,
    [NodesOf<Vec<writer::Writer>>],
    first_edge(vec![lead_row()])
);
answering!(
    nodes_of_a_deque,
    [NodesOf<VecDeque<writer::Writer>>],
    first_edge(VecDeque::from([lead_row()]))
);
answering!(
    nodes_of_a_linked_list,
    [NodesOf<LinkedList<writer::Writer>>],
    first_edge(LinkedList::from([lead_row()]))
);
answering!(
    nodes_of_a_sorted_set,
    [NodesOf<BTreeSet<writer::Writer>>],
    first_edge(BTreeSet::from([lead_row()]))
);
answering!(
    nodes_of_a_hashed_set,
    [NodesOf<HashSet<writer::Writer>>],
    first_edge(HashSet::from([lead_row()]))
);
answering!(
    nodes_of_a_boxed_slice,
    [NodesOf<Box<[writer::Writer]>>],
    first_edge(vec![lead_row()].into_boxed_slice())
);
answering!(
    nodes_of_a_shared_slice,
    [NodesOf<Arc<[writer::Writer]>>],
    first_edge(Arc::from([lead_row()]))
);
answering!(
    nodes_of_an_array,
    [NodesOf<[writer::Writer; 1]>],
    first_edge([lead_row()])
);
answering!(
    nodes_of_a_cow,
    [NodesOf<Cow<'static, writer::Writer>>],
    first_edge(Cow::Owned(lead_row()))
);
answering!(
    nodes_of_a_result,
    [NodesOf<Result<writer::Writer, GqlError>>],
    first_edge(Ok(lead_row()))
);
answering!(
    nodes_of_a_mutex,
    [NodesOf<tokio::sync::Mutex<writer::Writer>>],
    first_edge(tokio::sync::Mutex::new(lead_row()))
);
answering!(
    nodes_of_a_read_write_lock,
    [NodesOf<tokio::sync::RwLock<writer::Writer>>],
    first_edge(tokio::sync::RwLock::new(lead_row()))
);
answering!(
    nodes_of_a_list_of_options,
    [NodesOf<Vec<Option<writer::Writer>>>],
    first_edge(vec![Some(lead_row()), None])
);
answering!(
    nodes_of_a_json_list,
    [NodesOf<Json<Vec<writer::Writer>>>],
    first_edge(Json(vec![lead_row()]))
);
answering!(
    nodes_of_a_sorted_map,
    [NodesOf<BTreeMap<String, writer::Writer>>],
    first_edge(BTreeMap::from([("first".to_owned(), lead_row())]))
);
answering!(
    nodes_of_a_hashed_map,
    [NodesOf<HashMap<String, writer::Writer>>],
    first_edge(HashMap::from([("first".to_owned(), lead_row())]))
);
answering!(
    nodes_of_a_json_map,
    [NodesOf<Json<BTreeMap<String, writer::Writer>>>],
    first_edge(Json(BTreeMap::from([("first".to_owned(), lead_row())])))
);
answering!(
    nodes_of_a_json_array,
    [NodesOf<Json<[writer::Writer; 1]>>],
    first_edge(Json([lead_row()]))
);

/// What a flatten and a merge copy of an object whose column a field resolver serves.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct Cast {
    role: String,
    #[graphql(skip)]
    star: writer::Writer,
}

fn cast() -> Cast {
    Cast {
        role: "lead".to_owned(),
        star: lead_row(),
    }
}

#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct FlatCast {
    #[graphql(flatten)]
    cast: Cast,
}

#[derive(MergedObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct MergedCast(Cast, Footnote);

#[derive(SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct CastCopies {
    flat: FlatCast,
    merged: MergedCast,
}

answering!(
    copying_a_resolved_column,
    [CastCopies],
    CastCopies {
        flat: FlatCast { cast: cast() },
        merged: MergedCast(
            cast(),
            Footnote {
                text: "a footnote".to_owned(),
            },
        ),
    },
    {
        #[field_resolver]
        async fn star(&self, parent: &Cast) -> writer::Writer {
            parent.star.clone()
        }
    }
);

/// A column a flatten copied, served by a field resolver.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct CopiedCast {
    role: String,
    #[graphql(skip)]
    star: writer::Writer,
}

/// The same name as what it sits beside, holding the row itself.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct RawStar {
    star: writer::Writer,
}

#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct CopyBesideRaw {
    #[graphql(flatten)]
    cast: CopiedCast,
    raw: RawStar,
}

answering!(
    a_raw_column_beside_a_copied_one,
    [CopyBesideRaw],
    CopyBesideRaw {
        cast: CopiedCast {
            role: "lead".to_owned(),
            star: lead_row(),
        },
        raw: RawStar { star: lead_row() },
    },
    {
        #[field_resolver]
        async fn star(&self, parent: &CopiedCast) -> writer::Writer {
            parent.star.clone()
        }
    }
);

/// What the page and the decision name as passing: a row flattened beside a masked field
/// answering its own type leaves a link, so no reading of the schema tells it from a
/// column the resolver alone serves.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct FlatBesideMasked {
    #[graphql(flatten)]
    row: writer::Writer,
}

answering!(
    a_flatten_beside_a_masked_field,
    [FlatBesideMasked],
    FlatBesideMasked { row: lead_row() },
    {
        #[field_resolver]
        async fn also(&self, parent: &FlatBesideMasked) -> writer::Writer {
            parent.row.clone()
        }
    }
);

/// What the page and the decision name as passing: a derive answers its own column
/// first, and the resolver's name hides it from the schema.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct RenamedOntoItsResolver {
    #[graphql(name = "star")]
    raw_star: writer::Writer,
}

answering!(
    a_column_renamed_onto_its_resolver,
    [RenamedOntoItsResolver],
    RenamedOntoItsResolver {
        raw_star: lead_row()
    },
    {
        #[field_resolver]
        async fn star(&self, parent: &RenamedOntoItsResolver) -> writer::Writer {
            parent.raw_star.clone()
        }
    }
);

/// What the page and the decision name as passing: a scalar serializes what a body put
/// in it, and its type names no wire type.
#[derive(serde::Serialize)]
struct BodysOwnScalar {
    lead: writer::Writer,
}

answering!(
    a_scalar_a_body_builds,
    [Json<BodysOwnScalar>],
    Json(BodysOwnScalar { lead: lead_row() })
);

/// The same, as a column of an object: the schema reads a scalar there too.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct ScalarColumn {
    lead: Json<writer::Writer>,
}

answering!(
    a_scalar_column_of_an_object,
    [ScalarColumn],
    ScalarColumn {
        lead: Json(lead_row()),
    }
);

/// What `module`'s boot says, or, where it boots, what its answer ships to a member
/// whose grant withholds the column `query` selects.
async fn assert_refused_at_boot<M>(query: &str, holds: &[&str])
where
    M: nest_rs_core::Module + 'static,
{
    let built = crate::over_an_unconnected_pool()
        .module::<M>()
        .build()
        .await;
    let refusal = match built {
        Err(refusal) => format!("{refusal:#}"),
        Ok(app) => panic!(
            "boots, and answers {query} with {}",
            answer(&app, "member", query).await
        ),
    };
    assert!(refusal.contains("`Parent.answer`"), "{refusal}");
    for held in holds {
        assert!(refusal.contains(held), "{held}: {refusal}");
    }
    assert!(refusal.contains("#[graphql(skip)]"), "{refusal}");
    assert!(refusal.contains("#[field_resolver]"), "{refusal}");
}

/// No mask reads a row an object holds: it names no entity, so a field's answer can
/// neither be walked nor cut. The boot says so, whatever role would have asked.
#[tokio::test]
async fn an_object_holding_a_wire_type_ships_no_withheld_column() {
    let lead = "{ parent { answer { lead { email } } } }";
    assert_refused_at_boot::<holding::AnsweringModule>(lead, &["`Holder.lead`", "`Writer`"]).await;
    assert_refused_at_boot::<holding_in_a_list::AnsweringModule>(lead, &["`Holder.lead`"]).await;
    assert_refused_at_boot::<holding_by_hand::AnsweringModule>(lead, &["`WrittenHolder.lead`"])
        .await;
    assert_refused_at_boot::<holding_deeper::AnsweringModule>(
        "{ parent { answer { inner { lead { email } } } } }",
        &["`Holder.lead`"],
    )
    .await;
}

#[tokio::test]
async fn an_object_taking_a_wire_types_columns_as_its_own_ships_no_withheld_column() {
    let email = "{ parent { answer { email } } }";
    assert_refused_at_boot::<holding_flat::AnsweringModule>(email, &["`Writer`"]).await;
    assert_refused_at_boot::<holding_merged::AnsweringModule>(email, &["`Writer`"]).await;
}

#[tokio::test]
async fn a_union_or_an_interface_holding_a_wire_type_ships_no_withheld_column() {
    assert_refused_at_boot::<holding_in_a_union::AnsweringModule>(
        "{ parent { answer { ... on Writer { email } } } }",
        &["`HeldUnion`", "`Writer`"],
    )
    .await;
    assert_refused_at_boot::<holding_in_an_interface::AnsweringModule>(
        "{ parent { answer { email } } }",
        &["`HeldInterface`", "`Writer`"],
    )
    .await;
}

#[tokio::test]
async fn a_connection_the_mask_cannot_read_ships_no_withheld_column() {
    assert_refused_at_boot::<an_edge::AnsweringModule>(
        "{ parent { answer { node { email } } } }",
        &["`WriterEdge.node`"],
    )
    .await;
    let nodes = "{ parent { answer { nodes { email } } } }";
    assert_refused_at_boot::<a_shared_connection::AnsweringModule>(nodes, &["`Writer`"]).await;
    assert_refused_at_boot::<a_connection_of_holders::AnsweringModule>(
        "{ parent { answer { nodes { lead { email } } } } }",
        &["`Holder.lead`"],
    )
    .await;
    assert_refused_at_boot::<a_connection_of_flattened_holders::AnsweringModule>(
        "{ parent { answer { nodes { lead { email } } } } }",
        &["`FlatOfHolder.lead`"],
    )
    .await;
}

/// What a connection and its edges hold beside their nodes is an object of the body's,
/// which no mask reads, wherever the connection sits.
#[tokio::test]
async fn what_a_connection_holds_beside_its_nodes_ships_no_withheld_column() {
    assert_refused_at_boot::<a_connection_holding_a_row_beside_its_edges::AnsweringModule>(
        "{ parent { answer { lead { email } } } }",
        &["`Holder.lead`"],
    )
    .await;
    assert_refused_at_boot::<an_edge_holding_a_row_beside_its_node::AnsweringModule>(
        "{ parent { answer { edges { lead { email } } } } }",
        &["`Holder.lead`"],
    )
    .await;
    assert_refused_at_boot::<a_page_of_a_list_holding_a_row_beside_its_edges::AnsweringModule>(
        "{ parent { answer { lead { email } } } }",
        &["`Holder.lead`"],
    )
    .await;
}

/// A connection holding nothing beside its nodes boots, and masks as it did.
#[tokio::test]
async fn a_connection_holding_nothing_beside_its_nodes_masks_them() {
    let app = booted::<a_connection_holding_nothing_beside_its_edges::AnsweringModule>().await;

    let read = answer(&app, "member", "{ parent { answer { nodes { email } } } }").await;

    assert_forbidden(&read, &["email"]);
    assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
}

async fn booted<M>() -> TestApp
where
    M: nest_rs_core::Module + 'static,
{
    crate::over_an_unconnected_pool()
        .module::<M>()
        .build()
        .await
        .expect("boots")
}

/// The query selecting `selection` of the one field an `answering!` app serves.
fn of_the_answer(selection: &str) -> String {
    format!("{{ parent {{ answer {{ {selection} }} }} }}")
}

/// A node a client selects into: its selection masks each row the node holds, and what
/// a refused row leaves of its edges is `outside`.
async fn assert_node_masked<M>(outside: Value)
where
    M: nest_rs_core::Module + 'static,
{
    let app = booted::<M>().await;

    let read = answer(
        &app,
        "member",
        &of_the_answer("nodes { name } edges { cursor node { name } }"),
    )
    .await;
    assert!(read.get("errors").is_none(), "{read}");
    assert!(read.to_string().contains("Ada"), "{read}");

    for selection in ["nodes { name email }", "edges { node { name email } }"] {
        let read = answer(&app, "member", &of_the_answer(selection)).await;
        assert_forbidden(&read, &["email"]);
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
    }

    let read = answer(&app, "reader", &of_the_answer("nodes { email }")).await;
    assert!(read.get("errors").is_none(), "{read}");
    assert!(read.to_string().contains(SECRET_EMAIL), "{read}");

    let read = answer(&app, "outsider", &of_the_answer("edges { node { id } }")).await;
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["parent"]["answer"]["edges"], outside, "{read}");
}

/// A node a client selects nothing into: a scalar serializing whole, so every column the
/// mask takes refuses it, and what a refused row leaves of its edges is `outside`.
async fn assert_scalar_node_masked<M>(outside: Value)
where
    M: nest_rs_core::Module + 'static,
{
    let app = booted::<M>().await;

    for selection in ["nodes", "edges { node }"] {
        let read = answer(&app, "member", &of_the_answer(selection)).await;
        assert_forbidden(&read, &["circle", "email", "nickname"]);
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
        assert!(!read.to_string().contains("countess"), "{read}");
    }

    let read = answer(&app, "reader", &of_the_answer("nodes")).await;
    assert!(read.get("errors").is_none(), "{read}");
    assert!(read.to_string().contains(SECRET_EMAIL), "{read}");

    let read = answer(&app, "outsider", &of_the_answer("edges { node }")).await;
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(read["data"]["parent"]["answer"]["edges"], outside, "{read}");
}

/// What a list or a set holds inside a node is masked as a list's rows are: a refused row
/// leaves the node, which stays.
#[tokio::test]
async fn a_connection_whose_nodes_list_a_row_masks_it() {
    let logs = LogCapture::install();
    let empty = || json!([{ "node": [] }]);

    assert_node_masked::<nodes_of_a_vec::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_deque::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_linked_list::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_sorted_set::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_hashed_set::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_boxed_slice::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_shared_slice::AnsweringModule>(empty()).await;
    assert_node_masked::<nodes_of_a_list_of_options::AnsweringModule>(
        json!([{ "node": [Value::Null] }]),
    )
    .await;
    assert_no_error_line(&logs);
}

/// One row a node holds alone, or an array has no place to drop, leaves with its edge.
#[tokio::test]
async fn a_connection_whose_nodes_hold_one_row_masks_it() {
    let logs = LogCapture::install();

    assert_node_masked::<nodes_of_an_array::AnsweringModule>(json!([])).await;
    assert_node_masked::<nodes_of_a_cow::AnsweringModule>(json!([])).await;
    assert_node_masked::<nodes_of_a_result::AnsweringModule>(json!([])).await;
    assert_node_masked::<nodes_of_a_mutex::AnsweringModule>(json!([])).await;
    assert_node_masked::<nodes_of_a_read_write_lock::AnsweringModule>(json!([])).await;
    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_connection_whose_nodes_are_a_scalar_of_rows_refuses_every_column_cut() {
    let logs = LogCapture::install();

    assert_scalar_node_masked::<nodes_of_a_json_list::AnsweringModule>(json!([{ "node": [] }]))
        .await;
    assert_scalar_node_masked::<nodes_of_a_sorted_map::AnsweringModule>(json!([{ "node": {} }]))
        .await;
    assert_scalar_node_masked::<nodes_of_a_hashed_map::AnsweringModule>(json!([{ "node": {} }]))
        .await;
    assert_scalar_node_masked::<nodes_of_a_json_map::AnsweringModule>(json!([{ "node": {} }]))
        .await;
    assert_scalar_node_masked::<nodes_of_a_json_array::AnsweringModule>(json!([])).await;
    assert_no_error_line(&logs);
}

/// A column a flatten or a merge copies reads through the object it copied, so the field
/// resolver of that object masks it.
#[tokio::test]
async fn a_copy_of_a_column_a_field_resolver_serves_masks_it() {
    let app = booted::<copying_a_resolved_column::AnsweringModule>().await;

    let read = answer(
        &app,
        "member",
        &of_the_answer("flat { role star { name } } merged { text star { name } }"),
    )
    .await;
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["parent"]["answer"]["flat"]["star"]["name"],
        "Ada"
    );
    assert_eq!(
        read["data"]["parent"]["answer"]["merged"]["star"]["name"],
        "Ada"
    );

    for column in ["flat { star { email } }", "merged { star { email } }"] {
        let read = answer(&app, "member", &of_the_answer(column)).await;
        assert_forbidden(&read, &["email"]);
        assert!(!read.to_string().contains(SECRET_EMAIL), "{read}");
    }
}

/// A column of another object holding the row by the same name is no copy of it.
#[tokio::test]
async fn a_raw_column_beside_a_copied_one_ships_no_withheld_column() {
    assert_refused_at_boot::<a_raw_column_beside_a_copied_one::AnsweringModule>(
        "{ parent { answer { raw { star { email } } } } }",
        &["`RawStar.star`"],
    )
    .await;
}

/// What the page and the decision name as passing, pinned so a change closing one reads
/// here: each boots and ships what a grant withholds, on the body's own line.
#[tokio::test]
async fn what_the_page_names_as_passing_passes() {
    for (app, selection) in [
        (
            booted::<a_flatten_beside_a_masked_field::AnsweringModule>().await,
            of_the_answer("email also { name }"),
        ),
        (
            booted::<a_column_renamed_onto_its_resolver::AnsweringModule>().await,
            of_the_answer("star { email }"),
        ),
        (
            booted::<a_scalar_a_body_builds::AnsweringModule>().await,
            "{ parent { answer } }".to_owned(),
        ),
        (
            booted::<a_scalar_column_of_an_object::AnsweringModule>().await,
            of_the_answer("lead"),
        ),
    ] {
        let read = answer(&app, "member", &selection).await;
        assert!(read.to_string().contains(SECRET_EMAIL), "{read}");
    }
}
