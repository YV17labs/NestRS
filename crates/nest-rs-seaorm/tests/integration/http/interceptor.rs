//! Covers `src/http/interceptor.rs` under the HTTP edge: the request deadline
//! the edge sets reaches `DbContext`, which cuts the handler alone, so a commit
//! outliving the deadline answers the handler's own status.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::module;
use nest_rs_database::{Executor, OpenUnit, Settlement, Unhonoured, UnitOfWork, UnitSpec};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use sea_orm::DatabaseConnection;
use tokio::time::Instant;

/// A binding whose commit takes two seconds, on a task of its own.
struct SlowCommit;

struct SlowCommitUnit;

struct Nowhere;

impl Executor for Nowhere {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl UnitOfWork for SlowCommit {
    fn open(&self, _: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        Ok(Box::new(SlowCommitUnit))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

impl OpenUnit for SlowCommitUnit {
    fn executor(&self) -> Arc<dyn Executor> {
        Arc::new(Nowhere)
    }

    fn settle(self: Box<Self>, _ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        let committed = tokio::spawn(tokio::time::sleep(Duration::from_secs(2)));
        Box::pin(async move {
            drop(committed.await);
            Settlement::Settled
        })
    }
}

#[controller(path = "/probe")]
struct ProbeController;

#[routes]
impl ProbeController {
    #[post("/quick")]
    async fn quick(&self) -> StatusCode {
        tokio::time::sleep(Duration::from_millis(500)).await;
        StatusCode::CREATED
    }

    #[post("/slow")]
    async fn slow(&self) -> StatusCode {
        tokio::time::sleep(Duration::from_secs(30)).await;
        StatusCode::CREATED
    }
}

#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            request_timeout: Some(Duration::from_secs(1)),
            ..HttpConfig::default()
        }),
        SeaOrmDatabaseModule,
    ],
    providers = [ProbeController],
)]
struct ProbeModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .provide(DatabaseConnection::default())
        .provide(SeaOrmConfig::default())
        .provide_dyn::<dyn UnitOfWork>(Arc::new(SlowCommit))
        .module::<ProbeModule>()
        .build()
        .await
        .expect("the binding boots over a seeded pool and unit of work")
}

#[tokio::test(start_paused = true)]
async fn a_post_whose_commit_outlives_the_request_timeout_answers_its_own_status() {
    let app = boot().await;
    let started = Instant::now();
    let resp = app.http().post("/probe/quick").send().await;
    resp.assert_status(StatusCode::CREATED);
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(2500),
        "the commit was waited for past the deadline, never cut"
    );
}

#[tokio::test(start_paused = true)]
async fn a_handler_past_the_request_timeout_is_cut_and_answered_503() {
    let app = boot().await;
    let started = Instant::now();
    let resp = app.http().post("/probe/slow").send().await;
    resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    resp.assert_header(poem::http::header::RETRY_AFTER, "1");
    assert_eq!(started.elapsed(), Duration::from_secs(1));
}

/// A binding that journals the unit and policy of every unit it opens.
#[derive(Default)]
struct Journaling(std::sync::Mutex<Vec<String>>);

impl Journaling {
    fn read(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl UnitOfWork for Journaling {
    fn open(&self, spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        let policy = match spec.policy {
            nest_rs_database::TransactionPolicy::PerUnit => "per_unit",
            _ => "pool",
        };
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{} {policy}", spec.unit.name()));
        Ok(Box::new(Journaled))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

/// A unit [`Journaling`] opened, settling nothing.
struct Journaled;

impl OpenUnit for Journaled {
    fn executor(&self) -> Arc<dyn Executor> {
        Arc::new(Nowhere)
    }

    fn settle(self: Box<Self>, _ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        Box::pin(std::future::ready(Settlement::Settled))
    }
}

#[nest_rs_graphql::resolver]
struct WritesResolver;

#[nest_rs_graphql::operations]
impl WritesResolver {
    #[query]
    #[public]
    async fn read(&self) -> bool {
        true
    }

    #[mutation]
    #[public]
    async fn write(&self) -> bool {
        true
    }
}

#[module(
    imports = [
        HttpModule::for_root(None),
        SeaOrmDatabaseModule,
        nest_rs_graphql::GraphqlModule::for_root(None),
    ],
    providers = [WritesResolver, ProbeController],
)]
struct GraphqlProbeModule;

/// `/graphql` runs on the pool whatever its method, and each operation opens
/// its own unit: one boundary decides, never the request's transaction.
#[tokio::test(start_paused = true)]
async fn a_graphql_request_runs_on_the_pool_and_each_operation_opens_its_own_unit() {
    let journal = Arc::new(Journaling::default());
    let app = TestApp::builder()
        .provide(DatabaseConnection::default())
        .provide(SeaOrmConfig::default())
        .provide_dyn::<dyn UnitOfWork>(Arc::clone(&journal) as Arc<dyn UnitOfWork>)
        .module::<GraphqlProbeModule>()
        .build()
        .await
        .expect("the binding boots over a seeded pool and unit of work");

    for query in ["{ read }", "mutation { write }"] {
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": query }))
            .send()
            .await
            .assert_status_is_ok();
    }
    app.http()
        .post("/probe/quick")
        .send()
        .await
        .assert_status(StatusCode::CREATED);

    assert_eq!(
        journal.read(),
        [
            "http.request pool",
            "graphql.operation pool",
            "http.request pool",
            "graphql.operation per_unit",
            "http.request per_unit",
        ]
    );
}
