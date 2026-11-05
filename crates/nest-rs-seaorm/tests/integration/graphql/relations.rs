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

use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, injectable, module};
use nest_rs_graphql::async_graphql::connection::Connection;
use nest_rs_graphql::async_graphql::dataloader::DataLoader;
use nest_rs_graphql::async_graphql::{Error as GqlError, Json, Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, GraphqlView, dataloader, operations, resolver};
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
}

impl PkLoadable for writer::Entity {
    type Loader = RowsWriters;
}

impl RelatedTo<shelf::Entity> for novel::Entity {
    type Loader = RowsNovels;
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

/// The parent a hand-written field resolver answers under.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct Reading {
    id: i32,
}

/// A wire type's `Result` behind another name.
type Answered<T> = Result<T, ServiceError>;

/// Every shape of an answer holding a wire type's rows that a client selects into.
const WIRE_ANSWERS: [&str; 17] = [
    "lead",
    "bareLead",
    "renamedLead",
    "boxedLead",
    "sharedLead",
    "borrowedLead",
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

    #[query]
    #[public]
    async fn reading(&self) -> Reading {
        Reading { id: 1 }
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
    logs.expect_one("nest_rs::authz", "response masking failed");
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

    #[field_resolver]
    async fn lead(&self, _parent: &UngrantedReading) -> GqlResult<Option<writer::Writer>> {
        Ok(Some(lead_row()))
    }
}

#[module(
    imports = [GraphqlModule, SeaOrmDatabaseModule],
    providers = [BooksService, ShelvesService, Rows, UngrantedLibraryResolver],
)]
struct UngrantedLibraryModule;

/// An app attaching no ability has no grant to apply, and its own reads refuse.
#[tokio::test]
async fn a_field_resolver_answers_as_it_did_where_no_ability_is_attached() {
    let app = crate::over_an_unconnected_pool()
        .module::<UngrantedLibraryModule>()
        .build()
        .await
        .expect("boots");

    let read = answer(&app, "", "{ ungrantedReading { lead { email } } }").await;

    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["ungrantedReading"]["lead"]["email"],
        SECRET_EMAIL
    );
}
