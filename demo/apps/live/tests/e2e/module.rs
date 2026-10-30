use features::authn::AuthnGuard;
use features::testing::{ASSISTANT, client, token_for};
use live::LiveModule;
use nest_rs::guards::guard;
use nest_rs::http::StatusCode;
use nest_rs::testing::{EphemeralDatabase, TestApp};
use nest_rs::ws::CloseCode;

use super::harness::{ORG_ID, machine_token};

#[tokio::test]
async fn the_live_app_requires_its_own_audience_read_from_its_own_file() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .use_guards_global([guard::<AuthnGuard>()])
        .module::<LiveModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
        .build_ws()
        .await
        .expect("LiveModule serves on the configuration `nestrs run dev` reads");

    let refused = app
        .socket("/ws")
        .bearer(&token_for(ASSISTANT, ORG_ID, "user", client()))
        .expect_refused()
        .await;
    assert_eq!(refused, StatusCode::UNAUTHORIZED);
    let mut socket = app.socket("/ws").bearer(&machine_token()).connect().await;

    socket.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}
