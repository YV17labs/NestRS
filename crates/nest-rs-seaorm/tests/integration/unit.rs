//! Covers `src/unit.rs` under the HTTP edge: SeaORM's unit of work waits for
//! the work a request held for its commit until the request's deadline, never
//! past it, and lets it run on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::StatusCode;
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule};
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
