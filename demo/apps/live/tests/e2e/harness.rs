use features::testing::{claims, client};
use features::{Role, Subject};
use live::LiveModule;
use nest_rs::testing::ws::{WsApp, WsSocket};
use nest_rs::testing::{TestApp, TestAppBuilder};
use serde_json::Value;
use uuid::Uuid;

pub(crate) use features::testing::{ORG_ID, connection};

pub(crate) fn user_bearer(app: &WsApp, user: Uuid) -> String {
    app.bearer(&claims(ORG_ID, Role::User, Subject::User(user)))
}

pub(crate) fn machine_bearer(app: &WsApp) -> String {
    app.bearer(&claims(ORG_ID, Role::User, client()))
}

pub(crate) fn builder() -> TestAppBuilder {
    TestApp::builder()
        .module::<LiveModule>()
        .ephemeral_database::<migrations::Migrator>()
        .ephemeral_signing_key()
}

pub(crate) async fn boot() -> TestApp {
    builder()
        .build()
        .await
        .expect("LiveModule boots against a throwaway database")
}

pub(crate) async fn serve() -> WsApp {
    builder()
        .build_ws()
        .await
        .expect("LiveModule serves on a real port against a throwaway database")
}

pub(crate) async fn open(app: &WsApp, path: &str) -> WsSocket {
    open_as(app, path, &user_bearer(app, Uuid::now_v7())).await
}

pub(crate) async fn open_as(app: &WsApp, path: &str, authorization: &str) -> WsSocket {
    app.socket(path)
        .header("authorization", authorization)
        .connect()
        .await
}

pub(crate) async fn wait_for_presence(socket: &mut WsSocket, want: u64) {
    for _ in 0..50 {
        socket.send("presence", Value::Null).await;
        let frame = socket.next_envelope().await;
        assert_eq!(frame["event"], "presence");
        if frame["data"].as_u64().expect("presence count") == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("presence never reached {want}");
}
