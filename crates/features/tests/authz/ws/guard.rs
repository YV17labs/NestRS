//! `WsAuthGuard::can_activate` must fail closed when the connection's authz
//! state was not installed — the access-graph marker is only a *compile-time*
//! seam, this runtime check is what keeps a mis-wired gateway from serving
//! cross-tenant data through an unscoped `Repo` read.

use std::sync::Arc;

use features::authz::ws::WsAuthGuard;
use nestrs_authz::{with_ability, Ability};
use nestrs_ws::{MessageGuard, WsClient};
use serde_json::json;

#[tokio::test]
async fn rejects_when_no_ambient_ability_is_installed() {
    let guard = WsAuthGuard;
    let result = guard
        .can_activate(&WsClient::for_test(), "ping", &json!({}))
        .await;
    assert_eq!(result, Err("unauthenticated".into()));
}

#[tokio::test]
async fn allows_when_the_connection_captured_an_ability() {
    let guard = WsAuthGuard;
    let result = with_ability(Arc::new(Ability::default()), async {
        guard
            .can_activate(&WsClient::for_test(), "ping", &json!({}))
            .await
    })
    .await;
    assert_eq!(result, Ok(()));
}
