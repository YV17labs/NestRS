//! What `#[expose]` emits on a relation against Postgres: the batches read through
//! `Repo`, so SQL decides the rows, and each row its loader builds is judged there,
//! a column a `.fields` grant withholds reading `null` whatever the selection.
//! `tests/integration/graphql/relations.rs` holds the rule.

use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{injectable, module};
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_http::EndpointGuards;
use nest_rs_seaorm::{
    CrudService, Repo, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError,
};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

#[path = "../../harness/role_guard.rs"]
mod role_guard;

use role_guard::{RoleGuard, answer, assert_no_error_line};

const TABLE: &str = "relation_probe_books";

pub(crate) mod writer {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "ProbeWriter", service = super::WritersService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "relation_probe_writers")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        #[expose(nullable)]
        pub email: String,
        #[expose(nullable)]
        pub org: i32,
        #[sea_orm(has_many)]
        #[expose]
        pub books: HasMany<super::book::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod book {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "ProbeBook", service = super::BooksService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "relation_probe_books")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: String,
        #[expose(nullable)]
        pub isbn: String,
        #[expose]
        pub writer_id: Uuid,
        #[sea_orm(belongs_to, from = "writer_id", to = "id")]
        #[expose]
        pub writer: HasOne<super::writer::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

const ADA: &str = "01920000-0000-7000-8000-0000000001a1";
const BEA: &str = "01920000-0000-7000-8000-0000000001a2";
const FIRST_BOOK: &str = "01920000-0000-7000-8000-0000000001b1";
const ADA_EMAIL: &str = "ada@relation-probe.test";
const ISBN: &str = "978-relation-probe";

/// The rows every test reads: Ada of org 1 wrote two books, Bea of org 2 one.
async fn seed() {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        TABLE,
        &format!(
            "CREATE TABLE IF NOT EXISTS relation_probe_writers (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT NOT NULL,
                org INT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS {TABLE} (
                id UUID PRIMARY KEY,
                title TEXT NOT NULL,
                isbn TEXT NOT NULL,
                writer_id UUID NOT NULL REFERENCES relation_probe_writers(id)
             );
             INSERT INTO relation_probe_writers (id, name, email, org) VALUES
                ('{ADA}', 'Ada', '{ADA_EMAIL}', 1),
                ('{BEA}', 'Bea', 'bea@relation-probe.test', 2)
             ON CONFLICT (id) DO NOTHING;
             INSERT INTO {TABLE} (id, title, isbn, writer_id) VALUES
                ('{FIRST_BOOK}', 'Sketches', '{ISBN}', '{ADA}'),
                ('01920000-0000-7000-8000-0000000001b2', 'Notes', '{ISBN}', '{ADA}'),
                ('01920000-0000-7000-8000-0000000001b3', 'Letters', '{ISBN}', '{BEA}')
             ON CONFLICT (id) DO NOTHING;"
        ),
    )
    .await;
}

#[injectable]
#[derive(Default)]
pub(crate) struct WritersService;

impl CrudService for WritersService {
    type Entity = writer::Entity;
}

#[injectable]
#[derive(Default)]
pub(crate) struct BooksService;

impl CrudService for BooksService {
    type Entity = book::Entity;
}

fn ability_of(role: &str) -> nest_rs_authz::Ability {
    let mut ab = AbilityBuilder::new();
    match role {
        // A visitor's grants are the widest a test could give one.
        "" => {
            ab.can(Action::Read, book::Entity);
            ab.can(Action::Read, writer::Entity);
            return ab.build_visitor().expect("a valid rule set");
        }
        "member" => {
            ab.can(Action::Read, writer::Entity)
                .fields([writer::Column::Id, writer::Column::Name]);
            ab.can(Action::Read, book::Entity).fields([
                book::Column::Id,
                book::Column::Title,
                book::Column::WriterId,
            ]);
        }
        // Narrowly, and the writers of its own org alone.
        "org_member" => {
            ab.can(Action::Read, writer::Entity)
                .when(|p| p.eq(writer::Column::Org, 1))
                .fields([writer::Column::Id, writer::Column::Name]);
            ab.can(Action::Read, book::Entity);
        }
        // Every column of the writers of its own org.
        "org_reader" => {
            ab.can(Action::Read, writer::Entity)
                .when(|p| p.eq(writer::Column::Org, 1));
            ab.can(Action::Read, book::Entity);
        }
        _ => {
            ab.can(Action::Read, writer::Entity);
            ab.can(Action::Read, book::Entity);
        }
    }
    ab.build().expect("a valid rule set")
}

#[resolver]
struct ProbeResolver;

#[operations]
impl ProbeResolver {
    #[query]
    #[authorize(Read, book::Entity)]
    async fn probe_books(&self) -> Result<Vec<book::ProbeBook>, ServiceError> {
        let rows = Repo::<book::Entity>::all().await?;
        Ok(rows.iter().map(book::ProbeBook::from).collect())
    }

    #[query]
    #[authorize(Read, writer::Entity)]
    async fn probe_writers(&self) -> Result<Vec<writer::ProbeWriter>, ServiceError> {
        let rows = Repo::<writer::Entity>::all().await?;
        Ok(rows.iter().map(writer::ProbeWriter::from).collect())
    }

    /// Names a visitor's read of books, and of no writer.
    #[query]
    #[public(Read, book::Entity)]
    async fn public_probe_books(&self) -> Result<Vec<book::ProbeBook>, ServiceError> {
        let rows = Repo::<book::Entity>::all().await?;
        Ok(rows.iter().map(book::ProbeBook::from).collect())
    }

    /// Names no read: it answers a value of its own.
    #[query]
    #[public]
    async fn bare_probe_book(&self) -> book::ProbeBook {
        book::ProbeBook::from(&book::Model {
            id: FIRST_BOOK.parse().expect("a key"),
            title: "Sketches".to_owned(),
            isbn: ISBN.to_owned(),
            writer_id: ADA.parse().expect("a key"),
        })
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        GraphqlModule,
    ],
    providers = [
        WritersService,
        BooksService,
        RoleGuard,
        EndpointGuards<GraphqlView, (RoleGuard,)>,
        ProbeResolver,
    ],
)]
struct ProbeModule;

async fn boot() -> TestApp {
    seed().await;
    TestApp::for_module::<ProbeModule>()
        .await
        .expect("the schema boots")
}

/// `answer` resolved with no error and holds no value its grant withholds.
#[track_caller]
fn assert_withheld(answer: &Value) {
    assert!(answer.get("errors").is_none(), "{answer}");
    let text = answer.to_string();
    assert!(
        !text.contains(ADA_EMAIL) && !text.contains(ISBN),
        "{answer}"
    );
}

/// The book seeded first, whichever order the rows came back in.
fn first_book(answer: &Value) -> &Value {
    answer["data"]["probeBooks"]
        .as_array()
        .unwrap_or_else(|| panic!("books: {answer}"))
        .iter()
        .find(|book| book["id"] == FIRST_BOOK)
        .unwrap_or_else(|| panic!("the first book: {answer}"))
}

/// The writer `id`, whichever order the rows came back in.
fn writer_named<'a>(answer: &'a Value, id: &str) -> &'a Value {
    answer["data"]["probeWriters"]
        .as_array()
        .unwrap_or_else(|| panic!("writers: {answer}"))
        .iter()
        .find(|writer| writer["id"] == id)
        .unwrap_or_else(|| panic!("writer {id}: {answer}"))
}

#[tokio::test]
async fn a_has_one_answers_what_its_grant_allows() {
    let logs = LogCapture::install();
    let app = boot().await;

    let served = answer(&app, "member", "{ probeBooks { id writer { id name } } }").await;
    assert!(served.get("errors").is_none(), "{served}");
    assert_eq!(first_book(&served)["writer"]["name"], "Ada", "{served}");

    let withheld = answer(&app, "member", "{ probeBooks { id writer { email } } }").await;
    assert_withheld(&withheld);
    assert_eq!(
        first_book(&withheld)["writer"],
        json!({ "email": null }),
        "{withheld}"
    );

    let whole = answer(&app, "reader", "{ probeBooks { id writer { email } } }").await;
    assert!(whole.get("errors").is_none(), "{whole}");
    assert_eq!(first_book(&whole)["writer"]["email"], ADA_EMAIL);

    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_has_many_answers_what_its_grant_allows() {
    let logs = LogCapture::install();
    let app = boot().await;

    let served = answer(
        &app,
        "member",
        "{ probeWriters { id books { edges { cursor node { id title } } } } }",
    )
    .await;
    assert!(served.get("errors").is_none(), "{served}");
    let edges = &writer_named(&served, ADA)["books"]["edges"];
    assert_eq!(edges[0]["node"]["title"], "Sketches", "{served}");
    assert_eq!(edges[0]["cursor"], FIRST_BOOK, "{served}");

    let withheld = answer(
        &app,
        "member",
        "{ probeWriters { id books { edges { node { isbn } } } } }",
    )
    .await;
    assert_withheld(&withheld);
    assert_eq!(
        writer_named(&withheld, ADA)["books"]["edges"][0]["node"]["isbn"],
        Value::Null,
        "{withheld}"
    );

    let whole = answer(
        &app,
        "reader",
        "{ probeWriters { id books { nodes { isbn } } } }",
    )
    .await;
    assert!(whole.get("errors").is_none(), "{whole}");
    assert_eq!(writer_named(&whole, ADA)["books"]["nodes"][0]["isbn"], ISBN);

    assert_no_error_line(&logs);
}

#[tokio::test]
async fn two_aliases_read_through_one_batch_each_answer_their_own_selection() {
    let logs = LogCapture::install();
    let app = boot().await;

    let read = answer(
        &app,
        "member",
        "{ probeBooks { id granted: writer { name } withheld: writer { email } } }",
    )
    .await;
    assert_withheld(&read);
    let book = first_book(&read);
    assert_eq!(book["granted"]["name"], "Ada", "{read}");
    assert_eq!(book["withheld"], json!({ "email": null }), "{read}");

    let read = answer(
        &app,
        "member",
        "{ probeWriters { id granted: books(first: 1) { nodes { title } } withheld: books(first: 1) { nodes { isbn } } } }",
    )
    .await;
    assert_withheld(&read);
    let writer = writer_named(&read, ADA);
    assert_eq!(writer["granted"]["nodes"][0]["title"], "Sketches", "{read}");
    assert_eq!(
        writer["withheld"]["nodes"][0]["isbn"],
        Value::Null,
        "{read}"
    );

    assert_no_error_line(&logs);
}

#[tokio::test]
async fn a_list_relation_pages_under_a_narrow_grant() {
    let logs = LogCapture::install();
    let app = boot().await;

    let first = answer(
        &app,
        "member",
        "{ probeWriters { id books(first: 1) { nodes { title } pageInfo { hasNextPage endCursor } } } }",
    )
    .await;
    assert!(first.get("errors").is_none(), "{first}");
    let page = &writer_named(&first, ADA)["books"];
    assert_eq!(page["nodes"][0]["title"], "Sketches", "{first}");
    assert_eq!(page["pageInfo"]["hasNextPage"], true, "{first}");
    assert_eq!(page["pageInfo"]["endCursor"], FIRST_BOOK, "{first}");

    let second = answer(
        &app,
        "member",
        &format!(
            r#"{{ probeWriters {{ id books(first: 1, after: "{FIRST_BOOK}") {{ nodes {{ title }} pageInfo {{ hasNextPage hasPreviousPage }} }} }} }}"#
        ),
    )
    .await;
    assert!(second.get("errors").is_none(), "{second}");
    let page = &writer_named(&second, ADA)["books"];
    assert_eq!(page["nodes"][0]["title"], "Notes", "{second}");
    assert_eq!(page["pageInfo"]["hasNextPage"], false, "{second}");
    assert_eq!(page["pageInfo"]["hasPreviousPage"], true, "{second}");

    assert_no_error_line(&logs);
}

/// SQL decides the row, under a grant narrowing its columns or not; nothing files
/// an error for a row the caller may not read.
#[tokio::test]
async fn a_row_outside_the_grant_reads_null() {
    let logs = LogCapture::install();
    let app = boot().await;

    for role in ["org_member", "org_reader"] {
        let read = answer(&app, role, "{ probeBooks { id writerId writer { id } } }").await;

        assert!(read.get("errors").is_none(), "{role}: {read}");
        let books = read["data"]["probeBooks"].as_array().expect("books");
        for book in books {
            let expected = if book["writerId"] == ADA {
                json!({ "id": ADA })
            } else {
                Value::Null
            };
            assert_eq!(book["writer"], expected, "{role}: {read}");
        }
    }

    assert_no_error_line(&logs);
}

/// A visitor's nested read answers nothing, whatever its root names and whatever
/// its grants: no line on a relation names what it reads.
#[tokio::test]
async fn a_visitors_relation_reads_nothing_under_a_root_naming_its_read_or_not() {
    let logs = LogCapture::install();
    let app = boot().await;

    let named = answer(&app, "", "{ publicProbeBooks { id writer { name } } }").await;
    assert!(named.get("errors").is_none(), "{named}");
    let books = named["data"]["publicProbeBooks"].as_array().expect("books");
    assert!(!books.is_empty(), "the root reads by the grant it names");
    for book in books {
        assert_eq!(book["writer"], Value::Null, "{named}");
    }

    let bare = answer(&app, "", "{ bareProbeBook { id writer { email } } }").await;
    assert!(bare.get("errors").is_none(), "{bare}");
    assert_eq!(
        bare["data"]["bareProbeBook"]["writer"],
        Value::Null,
        "{bare}"
    );

    assert_no_error_line(&logs);
}
