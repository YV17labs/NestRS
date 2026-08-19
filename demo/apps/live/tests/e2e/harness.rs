use features::Role;
use live::LiveModule;
use nest_rs::authn::JwtConfig;
use nest_rs::testing::{TestApp, TestAppBuilder, WsApp, WsSocket};
use serde_json::Value;
use uuid::Uuid;

pub(crate) use features::testing::{AUDIENCE, DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn test_token() -> String {
    token_for_org(Uuid::parse_str(ORG_ID).expect("valid org uuid"), Role::User).await
}

pub(crate) async fn token_for_org(org_id: Uuid, role: Role) -> String {
    features::testing::token(org_id, vec![role], None)
}

pub(crate) fn boot_builder() -> TestAppBuilder {
    TestApp::builder()
        .module::<LiveModule>()
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(AUDIENCE.into()),
            ..Default::default()
        })
}

pub(crate) async fn serve() -> WsApp {
    boot_builder()
        .build_ws()
        .await
        .expect("LiveModule serves on a real port")
}

pub(crate) async fn open(app: &WsApp, path: &str) -> WsSocket {
    app.socket(path).bearer(&test_token().await).connect().await
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
