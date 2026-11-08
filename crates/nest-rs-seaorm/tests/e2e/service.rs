//! `src/service.rs` against Postgres, over real rows: a `#[crud]` update under a grant
//! narrowing what it writes, booted on HTTP and GraphQL, where a client carries back
//! what its `PUT` holds and hears a change it may not make by the input's field; what
//! a replacement carries back stays out of the statement; and a create's re-check holds
//! the fresh row to a grant permitting each column it chose.

use std::sync::Arc;

use nest_rs_authz::__private::install_ability;
use nest_rs_authz::{Ability, AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::{code, injectable, module};
use nest_rs_database::__private::with_request_executor;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, resolver};
use nest_rs_http::{EndpointGuards, StatusCode, controller};
use nest_rs_seaorm::__private::SeaOrmExecutor;
use nest_rs_seaorm::{
    Creatable, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, Updatable, expose,
};
use nest_rs_testing::TestApp;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, Set,
};
use serde_json::{Value, json};

use crate::always_authenticated::AlwaysAuthenticated;

/// `work_status` is of two words and the table calls it otherwise, so each of a client's
/// spellings differs from the column a refusal starts from.
mod article {
    use super::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Article", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "service_probe_articles")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        pub org_id: Uuid,
        #[expose(input(create, update))]
        pub title: String,
        #[sea_orm(column_name = "review_state")]
        #[expose(input(create, update))]
        pub work_status: String,
        #[expose(input(create, update))]
        pub long_bio: Option<String>,
        pub touched: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C: ConnectionTrait>(mut self, _: &C, _: bool) -> Result<Self, DbErr> {
            self.touched = sea_orm::Set(1);
            Ok(self)
        }
    }
}

#[injectable]
#[derive(Default)]
struct ArticlesService;

impl CrudService for ArticlesService {
    type Entity = article::Entity;
}

impl Creatable for ArticlesService {
    type Create = article::CreateArticle;
}

impl Updatable for ArticlesService {
    type Update = article::UpdateArticle;
}

/// Reads an article whole, and writes its title alone.
#[injectable]
#[derive(Default)]
struct TitlesOnly;

impl AbilityFactory for TitlesOnly {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, article::Entity);
        ab.can(Action::Update, article::Entity)
            .fields([article::Column::Id, article::Column::Title]);
    }
}

#[controller(path = "/articles")]
#[use_guards(AlwaysAuthenticated, AbilityGuard<TitlesOnly>)]
struct ArticlesController {
    #[inject]
    svc: Arc<ArticlesService>,
}

#[nest_rs_http::crud(service = ArticlesService, ops = [update])]
impl ArticlesController {}

#[resolver]
struct ArticlesResolver {
    #[inject]
    svc: Arc<ArticlesService>,
}

#[nest_rs_graphql::crud(service = ArticlesService, ops = [get, update])]
impl ArticlesResolver {}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        ArticlesService,
        TitlesOnly,
        AbilityGuard<TitlesOnly>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<TitlesOnly>)>,
        ArticlesController,
        ArticlesResolver,
    ],
)]
struct ArticlesModule;

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "service_probe_articles",
        "CREATE TABLE IF NOT EXISTS service_probe_articles (
            id UUID PRIMARY KEY,
            org_id UUID NOT NULL,
            title TEXT NOT NULL,
            review_state TEXT NOT NULL,
            long_bio TEXT,
            touched INTEGER NOT NULL DEFAULT 0
        );",
    )
    .await;
    conn
}

async fn seed(conn: &DatabaseConnection, org_id: Uuid) -> article::Model {
    article::Entity::insert(article::ActiveModel {
        id: Set(Uuid::now_v7()),
        org_id: Set(org_id),
        title: Set("first".to_owned()),
        work_status: Set("draft".to_owned()),
        long_bio: Set(Some("as it stands".to_owned())),
        touched: Set(0),
    })
    .exec_with_returning(conn)
    .await
    .expect("seed an article")
}

async fn stored(conn: &DatabaseConnection, id: Uuid) -> article::Model {
    article::Entity::find_by_id(id)
        .one(conn)
        .await
        .expect("read an article")
        .expect("the article stands")
}

async fn boot() -> TestApp {
    TestApp::for_module::<ArticlesModule>()
        .await
        .expect("the app boots over live Postgres")
}

/// A `PUT` of `row`, its status and its body.
async fn put(app: &TestApp, row: &article::Model, body: Value) -> (StatusCode, Value) {
    let response = app
        .http()
        .put(format!("/articles/{}", row.id))
        .body_json(&body)
        .send()
        .await;
    let status = response.0.status();
    (status, response.json().await.value().deserialize())
}

/// `updateArticle` on `row`, selecting `selection`.
async fn update(app: &TestApp, row: &article::Model, input: Value, selection: &str) -> Value {
    let query = format!(
        "mutation($id: String!, $input: UpdateArticle!) {{ \
             updateArticle(id: $id, input: $input) {{ {selection} }} \
         }}"
    );
    app.http()
        .post("/graphql")
        .body_json(&json!({
            "query": query,
            "variables": { "id": row.id.to_string(), "input": input },
        }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize()
}

#[tokio::test]
async fn a_put_carrying_back_what_it_read_writes_what_its_grant_permits() {
    let conn = db().await;
    let app = boot().await;
    let row = seed(&conn, Uuid::now_v7()).await;

    let (status, body) = put(
        &app,
        &row,
        json!({ "title": "second", "work_status": "draft", "long_bio": "as it stands" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "id": row.id, "title": "second" }),
        "a write's reply shows that write's grant",
    );
    let after = stored(&conn, row.id).await;
    assert_eq!(after.title, "second");
    assert_eq!(after.work_status, row.work_status);
    assert_eq!(after.long_bio, row.long_bio);
}

#[tokio::test]
async fn a_put_changing_what_its_grant_withholds_names_each_field_and_the_row_stands() {
    let conn = db().await;
    let app = boot().await;
    let row = seed(&conn, Uuid::now_v7()).await;

    let (status, body) = put(
        &app,
        &row,
        json!({ "title": "second", "work_status": "published", "long_bio": "rewritten" }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], code::FORBIDDEN.as_str(), "{body}");
    assert_eq!(
        body["errors"],
        json!({
            "work_status": [{ "code": "forbidden" }],
            "long_bio": [{ "code": "forbidden" }],
        }),
        "each field as its client spells it: {body}",
    );
    let said = body.to_string();
    assert!(!said.contains("review_state"), "{said}");
    assert!(!said.contains("published"), "{said}");
    assert_eq!(stored(&conn, row.id).await, row);
}

#[tokio::test]
async fn a_mutation_carrying_back_what_it_read_writes_what_its_grant_permits() {
    let conn = db().await;
    let app = boot().await;
    let row = seed(&conn, Uuid::now_v7()).await;

    let body = update(
        &app,
        &row,
        json!({ "title": "second", "workStatus": "draft", "longBio": "as it stands" }),
        "id title",
    )
    .await;

    assert!(body.get("errors").is_none(), "{body}");
    assert_eq!(body["data"]["updateArticle"]["title"], "second", "{body}");
    let after = stored(&conn, row.id).await;
    assert_eq!(after.title, "second");
    assert_eq!(after.work_status, row.work_status);
    assert_eq!(after.long_bio, row.long_bio);
}

#[tokio::test]
async fn a_mutation_changing_what_its_grant_withholds_names_each_field_and_the_row_stands() {
    let conn = db().await;
    let app = boot().await;
    let row = seed(&conn, Uuid::now_v7()).await;

    let body = update(
        &app,
        &row,
        json!({ "title": "second", "workStatus": "published", "longBio": "rewritten" }),
        "id title",
    )
    .await;

    let extensions = &body["errors"][0]["extensions"];
    assert_eq!(extensions["code"], code::FORBIDDEN.as_str(), "{body}");
    assert_eq!(
        extensions["errors"],
        json!({
            "workStatus": [{ "code": "forbidden" }],
            "longBio": [{ "code": "forbidden" }],
        }),
        "each field as the schema spells it: {body}",
    );
    let said = body.to_string();
    assert!(!said.contains("review_state"), "{said}");
    assert!(!said.contains("published"), "{said}");
    assert_eq!(stored(&conn, row.id).await, row);
}

/// A write's reply shows that write's grant, so a selection past it refuses the field,
/// and the mutation's savepoint takes back the write it had made.
#[tokio::test]
async fn a_mutation_selecting_what_its_grant_withholds_rolls_the_write_back() {
    let conn = db().await;
    let app = boot().await;
    let row = seed(&conn, Uuid::now_v7()).await;

    let body = update(
        &app,
        &row,
        json!({ "title": "second", "workStatus": "draft", "longBio": "as it stands" }),
        "id workStatus",
    )
    .await;

    assert_eq!(
        body["errors"][0]["extensions"]["code"],
        code::FORBIDDEN.as_str(),
        "{body}",
    );
    assert_eq!(
        body["errors"][0]["extensions"]["fields"],
        json!(["workStatus"]),
        "{body}",
    );
    assert_eq!(stored(&conn, row.id).await, row);
}

/// `work`, in a request unit on the pool under `ability`.
async fn under<T>(conn: &DatabaseConnection, ability: Ability, work: impl Future<Output = T>) -> T {
    with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        install_ability(Arc::new(ability), work),
    )
    .await
}

fn granting(grants: impl FnOnce(&mut AbilityBuilder)) -> Ability {
    let mut b = AbilityBuilder::new();
    grants(&mut b);
    b.build().expect("valid test ability")
}

/// What a replacement carries back is no write: it stays out of the statement, and so
/// leaves what another writer put there since the caller read it.
#[tokio::test]
async fn a_replacement_leaves_what_it_carries_back_to_its_next_writer() {
    let conn = db().await;
    let row = seed(&conn, Uuid::now_v7()).await;
    let mut concurrent = row.clone().into_active_model();
    concurrent.work_status = Set("another writer's".to_owned());
    article::Entity::update(concurrent)
        .exec(&conn)
        .await
        .expect("another writer lands first");
    let ability = granting(|b| {
        b.can(Action::Read, article::Entity);
        b.can(Action::Update, article::Entity)
            .fields([article::Column::Title]);
    });

    let updated = under(
        &conn,
        ability,
        ArticlesService.update(
            row.clone(),
            article::UpdateArticle {
                title: "second".to_owned(),
                work_status: row.work_status.clone(),
                long_bio: row.long_bio.clone(),
            },
        ),
    )
    .await
    .expect("what it carries back stands as the caller read it");

    assert_eq!(updated.title, "second");
    assert_eq!(updated.work_status, "another writer's");
}

/// The grant's pin, the seeded key and the hook's stamp need no grant, nor a null
/// on a withheld column, a member the input did not carry.
#[tokio::test]
async fn a_narrowed_create_inserts_what_its_grant_permits_and_what_the_server_fills() {
    let conn = db().await;
    let org = Uuid::now_v7();
    let ability = granting(|b| {
        b.can(Action::Create, article::Entity)
            .when(|p| p.eq(article::Column::OrgId, org))
            .fields([article::Column::Title, article::Column::WorkStatus]);
    });

    let created = under(
        &conn,
        ability,
        ArticlesService.create(article::CreateArticle {
            title: "first".to_owned(),
            work_status: "draft".to_owned(),
            long_bio: None,
        }),
    )
    .await
    .expect("the grant permits what the input chose");

    assert_eq!(created.org_id, org);
    assert_eq!(created.title, "first");
    assert_eq!(created.touched, 1);
}

/// The re-check reads the columns' grants: a fresh row meeting the one grant permitting
/// `title`, and not the one permitting `work_status`, rolls back.
#[tokio::test]
async fn a_create_whose_row_its_grants_do_not_permit_each_column_rolls_back() {
    let conn = db().await;
    let (pinned, elsewhere) = (Uuid::now_v7(), Uuid::now_v7());
    let ability = granting(|b| {
        b.can(Action::Create, article::Entity)
            .when(|p| p.eq(article::Column::OrgId, pinned))
            .fields([article::Column::OrgId, article::Column::Title]);
        b.can(Action::Create, article::Entity)
            .when(|p| p.eq(article::Column::OrgId, elsewhere))
            .fields([article::Column::OrgId, article::Column::WorkStatus]);
    });

    let rolled_back = under(
        &conn,
        ability,
        ArticlesService.create_from_active(article::ActiveModel {
            id: Set(Uuid::now_v7()),
            org_id: Set(pinned),
            title: Set("first".to_owned()),
            work_status: Set("draft".to_owned()),
            ..Default::default()
        }),
    )
    .await;

    assert!(
        matches!(rolled_back, Err(DbErr::RecordNotInserted)),
        "{rolled_back:?}",
    );
    let inserted = article::Entity::find()
        .filter(article::Column::OrgId.eq(pinned))
        .count(&conn)
        .await
        .expect("count");
    assert_eq!(inserted, 0);
}
