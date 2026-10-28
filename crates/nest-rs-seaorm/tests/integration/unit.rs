//! Covers `src/unit.rs` under the HTTP edge: SeaORM's unit of work waits for
//! the work a request held for its commit until the request's deadline, never
//! past it, and lets it run on; and no handle a unit installs downcasts, through
//! the port's reader, to an executor a service could run a statement on.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::module;
use nest_rs_database::{AtomicError, atomic, current_executor};
use nest_rs_http::StatusCode;
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmExecutor};
use nest_rs_testing::TestApp;
use sea_orm::DatabaseConnection;
use tokio::time::Instant;

/// Whether the work the request held for its commit ran.
static LISTENED: AtomicBool = AtomicBool::new(false);

#[controller(path = "/probe")]
struct ProbeController;

#[routes]
impl ProbeController {
    #[post("/")]
    async fn write(&self) -> StatusCode {
        nest_rs_database::after_commit(async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            LISTENED.store(true, Ordering::SeqCst);
        })
        .await;
        tokio::time::sleep(Duration::from_millis(900)).await;
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

#[tokio::test(start_paused = true)]
async fn a_post_whose_listener_outlives_the_request_timeout_is_answered_at_the_deadline() {
    let app = TestApp::builder()
        .provide(DatabaseConnection::default())
        .provide(SeaOrmConfig::default())
        .module::<ProbeModule>()
        .build()
        .await
        .expect("the binding boots over a seeded pool");

    let started = Instant::now();
    let resp = app.http().post("/probe").send().await;
    resp.assert_status(StatusCode::CREATED);
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(1),
        "the listener was waited for until the deadline, and no longer"
    );
    assert!(
        !LISTENED.load(Ordering::SeqCst),
        "it runs on past the answer"
    );
    nest_rs_testing::wait_until(Duration::from_secs(5), || LISTENED.load(Ordering::SeqCst)).await;
}

/// Whether the running unit's handle downcasts, through the port's reader, to
/// the executor type a service can name — one it could run an unfiltered
/// statement on.
fn reachable() -> bool {
    current_executor()
        .expect("the handler runs in a unit")
        .as_any()
        .downcast_ref::<SeaOrmExecutor>()
        .is_some()
}

/// [`reachable`] inside an atomic block of the running unit.
async fn reachable_in_a_block() -> bool {
    atomic(async { Ok::<_, AtomicError>(reachable()) })
        .await
        .expect("a block that sends nothing settles")
}

/// What the work a write held for its commit read.
static HELD: Mutex<Option<bool>> = Mutex::new(None);

#[controller(path = "/sealed")]
struct SealedController;

#[routes]
impl SealedController {
    #[get("/")]
    async fn read(&self) -> String {
        format!("{} {}", reachable(), reachable_in_a_block().await)
    }

    #[post("/")]
    async fn write(&self) -> String {
        let outside = current_executor()
            .expect("the handler runs in a unit")
            .non_transactional()
            .expect("a unit that sent nothing steps out to its pool")
            .as_any()
            .downcast_ref::<SeaOrmExecutor>()
            .is_some();
        nest_rs_database::after_commit(async {
            *HELD.lock().unwrap() = Some(reachable());
        })
        .await;
        format!("{} {} {outside}", reachable(), reachable_in_a_block().await)
    }
}

#[module(
    imports = [HttpModule::for_root(None), SeaOrmDatabaseModule],
    providers = [SealedController],
)]
struct SealedModule;

#[tokio::test]
async fn no_handle_a_unit_installs_downcasts_to_an_executor_a_service_can_name() {
    let app = TestApp::builder()
        .provide(DatabaseConnection::default())
        .provide(SeaOrmConfig::default())
        .module::<SealedModule>()
        .build()
        .await
        .expect("the binding boots over a seeded pool");

    app.http()
        .get("/sealed")
        .send()
        .await
        .assert_text("false false")
        .await;
    app.http()
        .post("/sealed")
        .send()
        .await
        .assert_text("false false false")
        .await;
    assert_eq!(
        *HELD.lock().unwrap(),
        Some(false),
        "the pool held work runs on is sealed too"
    );
}
