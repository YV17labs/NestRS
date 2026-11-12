use features::Role;
use features::testing::{ASSISTANT, LIVE, claims, client, with_claim};
use nest_rs::http::StatusCode;
use nest_rs::ws::CloseCode;

use super::harness::{ORG_ID, open_as, serve};

#[tokio::test]
async fn an_anonymous_caller_is_refused_at_every_gateways_upgrade() {
    let app = serve().await;

    for gateway in ["/ws", "/users", "/notify"] {
        let refused = app.socket(gateway).expect_refused().await;
        assert_eq!(refused, StatusCode::UNAUTHORIZED, "{gateway}");
    }

    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn the_live_app_requires_its_own_audience_read_from_its_own_file() {
    let app = serve().await;
    let for_resource = |resource: &str| {
        app.bearer(&with_claim(
            claims(ORG_ID, Role::User, client()),
            "aud",
            resource,
        ))
    };

    let refused = app
        .socket("/ws")
        .header("authorization", for_resource(ASSISTANT))
        .expect_refused()
        .await;
    assert_eq!(refused, StatusCode::UNAUTHORIZED);
    let mut socket = open_as(&app, "/ws", &for_resource(LIVE)).await;

    socket.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}
