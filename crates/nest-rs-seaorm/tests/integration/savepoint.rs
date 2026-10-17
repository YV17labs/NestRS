//! Covers `src/savepoint.rs` under a GraphQL mutation: a root field's
//! savepoint is lazy, so a mutation that never touches the database sends
//! nothing — proved on a pool that reaches no database, where any statement
//! fails.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::HttpModule;
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule, ServiceError, current_executor};
use nest_rs_testing::TestApp;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};

#[resolver]
struct TouchResolver;

#[operations]
impl TouchResolver {
    #[query]
    #[public]
    async fn up(&self) -> bool {
        true
    }

    #[mutation]
    #[public]
    async fn touch_nothing(&self) -> bool {
        true
    }

    #[mutation]
    #[public]
    async fn touch(&self) -> Result<bool, ServiceError> {
        current_executor()
            .expect("a mutation runs on its unit's executor")
            .execute_unprepared("SELECT 1")
            .await?;
        Ok(true)
    }
}

#[module(
    imports = [HttpModule::for_root(None), SeaOrmDatabaseModule, GraphqlModule::for_root(None)],
    providers = [TouchResolver],
)]
struct TouchModule;

async fn mutate(field: &str) -> (poem::http::StatusCode, String) {
    let app = TestApp::builder()
        .provide(DatabaseConnection::default())
        .provide(SeaOrmConfig::default())
        .module::<TouchModule>()
        .build()
        .await
        .expect("the binding boots over a pool that reaches nothing");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": format!("mutation {{ {field} }}") }))
        .send()
        .await
        .0;
    let status = resp.status();
    let body = resp.into_body().into_string().await.expect("a body");
    (status, body)
}

#[tokio::test]
async fn a_mutation_that_never_touches_the_database_sends_no_savepoint() {
    let (status, body) = mutate("touchNothing").await;
    assert_eq!(status, poem::http::StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("a GraphQL body"),
        json!({ "data": { "touchNothing": true } }),
        "no BEGIN and no SAVEPOINT reached the pool"
    );

    let (status, _) = mutate("touch").await;
    assert_eq!(
        status,
        poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        "the control: SeaORM's disconnected pool fails any statement sent to it"
    );
}
