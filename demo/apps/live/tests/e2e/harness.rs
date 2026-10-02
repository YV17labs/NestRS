use features::Role;
use live::LiveModule;
use nest_rs::authn::AuthnConfig;
use nest_rs::testing::ws::{WsApp, WsSocket};
use nest_rs::testing::{EphemeralDatabase, TestApp, TestAppBuilder};
use serde_json::Value;
use uuid::Uuid;

pub(crate) use features::testing::{AUDIENCE, DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn test_token() -> String {
    token_for_org(Uuid::parse_str(ORG_ID).expect("valid org uuid"), Role::User).await
}

pub(crate) async fn token_for_org(org_id: Uuid, role: Role) -> String {
    features::testing::token(org_id, vec![role], None)
}

pub(crate) async fn boot_builder() -> (EphemeralDatabase, TestAppBuilder) {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let builder = TestApp::builder()
        .module::<LiveModule>()
        .provide_arc(db.connection())
        .provide(AuthnConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(AUDIENCE.into()),
            ..Default::default()
        });
    (db, builder)
}

pub(crate) async fn boot() -> (EphemeralDatabase, TestApp) {
    let (db, builder) = boot_builder().await;
    let app = builder
        .build()
        .await
        .expect("LiveModule boots against the throwaway database");
    (db, app)
}

pub(crate) async fn serve() -> (EphemeralDatabase, WsApp) {
    let (db, builder) = boot_builder().await;
    let app = builder
        .build_ws()
        .await
        .expect("LiveModule serves on a real port against the throwaway database");
    (db, app)
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
