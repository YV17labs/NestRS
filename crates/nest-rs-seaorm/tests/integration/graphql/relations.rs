//! Covers `src/graphql/relations.rs` and what `#[expose]` emits on a relation's
//! field: a relation answers its target's wire rows, each judged where its loader
//! built it, and follows a link only where the caller may read it; whatever shape a
//! body puts a row in, the row holds nothing its grant withholds. The batches below
//! hold their rows in memory where the emitted ones read `Repo`, which
//! `tests/e2e/graphql/relations.rs` proves.

use std::collections::HashMap;

use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{injectable, module};
use nest_rs_graphql::async_graphql::dataloader::DataLoader;
use nest_rs_graphql::async_graphql::{Json, Result as GqlResult, SimpleObject};
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
        #[expose(nullable)]
        pub email: String,
        #[expose]
        pub nickname: Option<String>,
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
        #[expose(nullable)]
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
        #[expose(nullable)]
        pub isbn: String,
        #[expose(nullable)]
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

const SECRET_EMAIL: &str = "withheld@example.test";
const SECRET_ISBN: &str = "978-withheld";

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
    }
}

/// The writer as a body builds it, judged for the caller answering.
fn lead_row() -> writer::Writer {
    writer::Writer::from(&writer_row())
}

fn book_row() -> book::Model {
    book::Model {
        id: Uuid::nil(),
        writer_id: writer_id(),
    }
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

/// The batches standing in for the emitted ones: each builds its wire rows, judged
/// under what the request carries into the batch.
#[injectable]
#[derive(Default)]
struct Rows;

#[dataloader]
impl Rows {
    async fn writers(&self, keys: &[Uuid]) -> Result<HashMap<Uuid, writer::Writer>, ServiceError> {
        Ok(keys
            .iter()
            .map(|key| (*key, writer::Writer::from(&writer_row())))
            .collect())
    }

    async fn novels(
        &self,
        keys: &[RelationKey<Uuid>],
    ) -> Result<HashMap<RelationKey<Uuid>, RelationPage<novel::Novel>>, ServiceError> {
        Ok(keys
            .iter()
            .map(|key| {
                let edges = novel_rows()
                    .iter()
                    .map(|row| {
                        (
                            PageCursor::from(row.id).to_string(),
                            novel::Novel::from(row),
                        )
                    })
                    .collect();
                (
                    key.clone(),
                    nest_rs_seaorm::__private::relation_page(edges, false),
                )
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

fn ability_of(role: &str) -> nest_rs_authz::Ability {
    let mut ab = AbilityBuilder::new();
    if role == "stranger" {
        ab.can(Action::Read, shelf::Entity)
            .when(|p| p.eq(shelf::Column::Id, Uuid::nil()));
    } else {
        ab.can(Action::Read, shelf::Entity);
    }
    match role {
        // Reads a writer's name and a novel's title, a book whole.
        "member" => {
            ab.can(Action::Read, book::Entity);
            ab.can(Action::Read, writer::Entity)
                .fields([writer::Column::Id, writer::Column::Name]);
            ab.can(Action::Read, novel::Entity)
                .fields([novel::Column::Id, novel::Column::Title]);
        }
        // Reads a book's key alone, so never the writer it links to.
        "linkless" => {
            ab.can(Action::Read, book::Entity)
                .fields([book::Column::Id]);
            ab.can(Action::Read, writer::Entity);
        }
        _ => {
            ab.can(Action::Read, book::Entity);
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

/// An object holding a writer and a note of its own.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct Holder {
    lead: writer::Writer,
    note: String,
}

/// A row flattened beside a field answering its own type.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct FlatBesideServed {
    #[graphql(flatten)]
    row: writer::Writer,
}

/// A column renamed onto its field resolver's name.
#[derive(Clone, SimpleObject)]
#[graphql(complex, crate = "::nest_rs_graphql::async_graphql")]
struct RenamedOntoItsResolver {
    #[graphql(name = "star")]
    raw_star: writer::Writer,
}

/// A scalar serializing what a body put in it.
#[derive(serde::Serialize)]
struct BodysOwnScalar {
    lead: writer::Writer,
}

/// A row as an object's scalar column, and in a map column.
#[derive(Clone, SimpleObject)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct ScalarColumns {
    lead: Json<writer::Writer>,
    by_name: Json<HashMap<String, writer::Writer>>,
}

#[resolver]
struct LibraryResolver;

#[operations]
impl LibraryResolver {
    #[query]
    #[authorize(Read, book::Entity)]
    async fn books(&self) -> GqlResult<Vec<book::Book>> {
        Ok(vec![book::Book::from(&book_row())])
    }

    /// Three parents, each reaching one writer through its relation.
    #[query]
    #[authorize(Read, book::Entity)]
    async fn three_books(&self) -> GqlResult<Vec<book::Book>> {
        Ok((0..3).map(|_| book::Book::from(&book_row())).collect())
    }

    #[query]
    #[authorize(Read, shelf::Entity)]
    async fn shelves(&self) -> GqlResult<Vec<shelf::Shelf>> {
        Ok(vec![shelf::Shelf::from(&shelf::Model { id: shelf_id() })])
    }

    /// A scalar a body answers off a row a generated loader read.
    #[query]
    #[authorize(Read, writer::Entity)]
    async fn lead_email(&self, writers: &DataLoader<RowsWriters>) -> GqlResult<Option<String>> {
        let row = writers
            .load_one(writer_id())
            .await
            .map_err(|failed| nest_rs_graphql::async_graphql::Error::new(failed.to_string()))?;
        Ok(row.and_then(|row| row.email))
    }

    #[query]
    #[public]
    async fn reading(&self) -> Reading {
        Reading { id: 1 }
    }

    #[query]
    #[authorize(Read, writer::Entity)]
    async fn holder(&self) -> Holder {
        Holder {
            lead: lead_row(),
            note: "held".to_owned(),
        }
    }

    #[query]
    #[authorize(Read, writer::Entity)]
    async fn flat(&self) -> FlatBesideServed {
        FlatBesideServed { row: lead_row() }
    }

    #[query]
    #[authorize(Read, writer::Entity)]
    async fn renamed(&self) -> RenamedOntoItsResolver {
        RenamedOntoItsResolver {
            raw_star: lead_row(),
        }
    }

    #[query]
    #[authorize(Read, writer::Entity)]
    async fn scalar(&self) -> Json<BodysOwnScalar> {
        Json(BodysOwnScalar { lead: lead_row() })
    }

    #[query]
    #[authorize(Read, writer::Entity)]
    async fn scalar_columns(&self) -> ScalarColumns {
        ScalarColumns {
            lead: Json(lead_row()),
            by_name: Json(HashMap::from([("Ada".to_owned(), lead_row())])),
        }
    }

    #[field_resolver]
    async fn lead(&self, _parent: &Reading) -> GqlResult<Option<writer::Writer>> {
        Ok(Some(lead_row()))
    }

    #[field_resolver]
    async fn also(&self, parent: &FlatBesideServed) -> writer::Writer {
        parent.row.clone()
    }

    #[field_resolver]
    async fn star(&self, parent: &RenamedOntoItsResolver) -> writer::Writer {
        parent.raw_star.clone()
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

#[tokio::test]
async fn a_has_one_answers_its_target_as_the_grant_reads_it() {
    let logs = LogCapture::install();
    let app = library().await;
    let read = answer(
        &app,
        "member",
        "{ books { writer { id name email nickname } } }",
    )
    .await;
    assert_eq!(
        read,
        json!({ "data": { "books": [{ "writer": {
            "id": writer_id().to_string(),
            "name": "Ada",
            "email": null,
            "nickname": null,
        } }] } }),
    );
    assert_no_error_line(&logs);

    let read = answer(&app, "admin", "{ books { writer { email } } }").await;
    assert_eq!(
        read["data"]["books"][0]["writer"]["email"], SECRET_EMAIL,
        "{read}"
    );
}

/// A link the grant withholds is no link to follow: the relation reads `null`.
#[tokio::test]
async fn a_has_one_whose_link_the_grant_withholds_reads_null() {
    let app = library().await;
    let read = answer(&app, "linkless", "{ books { id writerId writer { id } } }").await;
    assert_eq!(
        read,
        json!({ "data": { "books": [{ "id": Uuid::nil().to_string(), "writerId": null, "writer": null }] } }),
    );
}

#[tokio::test]
async fn a_has_many_answers_each_child_as_the_grant_reads_it() {
    let app = library().await;
    let read = answer(
        &app,
        "member",
        "{ shelves { novels { edges { node { id title isbn shelfId } } } } }",
    )
    .await;
    let edges = read["data"]["shelves"][0]["novels"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("a page: {read}"));
    assert_eq!(edges.len(), 2, "{read}");
    for edge in edges {
        assert_eq!(edge["node"]["isbn"], Value::Null, "{read}");
        assert_eq!(edge["node"]["shelfId"], Value::Null, "{read}");
        assert_eq!(edge["node"]["title"], "Sketches", "{read}");
    }
}

/// A parent its caller may not read lends no relation its key outside a write's
/// reply: its children are never loaded, though the caller may read them.
#[tokio::test]
async fn a_has_many_under_a_parent_its_caller_may_not_read_answers_nothing() {
    let app = library().await;
    let read = answer(
        &app,
        "stranger",
        "{ shelves { novels { edges { node { title } } } } }",
    )
    .await;
    assert_eq!(read["data"], Value::Null, "{read}");
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{read}"
    );
    assert!(!read.to_string().contains("Sketches"), "{read}");
}

/// A body reading a column off a row a generated loader answered reads what the
/// grant lets it: a withheld one is no value there.
#[tokio::test]
async fn a_scalar_a_body_reads_off_a_loaded_row_holds_what_the_grant_reads() {
    let app = library().await;
    let read = answer(&app, "member", "{ leadEmail }").await;
    assert_eq!(read, json!({ "data": { "leadEmail": null } }));
    let read = answer(&app, "admin", "{ leadEmail }").await;
    assert_eq!(read, json!({ "data": { "leadEmail": SECRET_EMAIL } }));
}

/// Every shape holding a row — a scalar a body builds, a scalar or a map column of
/// an object, a flatten beside a field answering its type, a column renamed onto
/// its resolver's name — holds it as judged, so none ships what the grant
/// withholds.
#[tokio::test]
async fn no_shape_holding_a_row_ships_what_its_grant_withholds() {
    let app = library().await;
    for selection in [
        "{ holder { lead { email } note } }",
        "{ flat { email also { email } } }",
        "{ renamed { star { email } } }",
        "{ scalar }",
        "{ scalarColumns { lead byName } }",
        "{ reading { lead { email } } }",
    ] {
        let read = answer(&app, "member", selection).await;
        assert!(read.get("errors").is_none(), "{selection}: {read}");
        assert!(
            !read.to_string().contains(SECRET_EMAIL),
            "{selection}: {read}"
        );
        let read = answer(&app, "admin", selection).await;
        assert!(
            read.to_string().contains(SECRET_EMAIL),
            "{selection}: {read}"
        );
    }
}

/// A relation's batch is shared by every parent asking: three parents, one line
/// for nothing refused, and each answers its own row.
#[tokio::test]
async fn every_parent_of_a_relation_answers_what_one_batch_judged() {
    let logs = LogCapture::install();
    let app = library().await;
    let read = answer(&app, "member", "{ threeBooks { writer { name email } } }").await;
    assert_eq!(
        read["data"]["threeBooks"],
        json!([
            { "writer": { "name": "Ada", "email": null } },
            { "writer": { "name": "Ada", "email": null } },
            { "writer": { "name": "Ada", "email": null } },
        ]),
        "{read}",
    );
    assert_no_error_line(&logs);
}
