//! `src/ctx.rs`: an `Ambient<T>` parameter reads what a guard attached — the
//! upgrade's guards for every message, a message's own for its handler — and
//! is never the payload; absent, the message answers opaquely and files the
//! type.

use nest_rs_core::{Ambient, AmbientValue, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::HttpView;
use nest_rs_testing::ws::{WsApp, WsSocket};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{WsModule, WsView, gateway, messages};

/// Who the upgrade's guard says is calling.
#[derive(Clone)]
struct Caller(String);

impl AmbientValue for Caller {}

/// What a message's own guard derives from the caller.
#[derive(Clone)]
struct Author(String);

impl AmbientValue for Author {}

/// What no guard attaches.
#[derive(Clone)]
struct Unattached;

impl AmbientValue for Unattached {}

/// Attaches the caller the upgrade names, for every message of the socket.
#[injectable]
#[derive(Default, Layer)]
struct CallerGuard;

#[async_trait]
impl Guard for CallerGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let name = HttpView::of(cx)?
            .request()
            .header("x-name")
            .unwrap_or("anonymous");
        let caller = Caller(name.to_owned());
        cx.attach(caller);
        Ok(())
    }
}

impl GuardFor<HttpView> for CallerGuard {}

/// Derives the author from the caller, at the message it is bound on.
#[injectable]
#[derive(Default, Layer)]
struct AuthorGuard;

#[async_trait]
impl Guard for AuthorGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let caller = cx.require::<Caller>()?;
        let author = Author(format!("{} (author)", caller.0));
        cx.attach(author);
        Ok(())
    }
}

impl GuardFor<WsView> for AuthorGuard {}

#[gateway(path = "/ctx")]
#[use_guards(CallerGuard)]
struct CtxGateway;

#[messages]
impl CtxGateway {
    #[subscribe_message("whoami")]
    #[public]
    async fn whoami(&self, caller: Ambient<Caller>) -> String {
        caller.into_inner().0
    }

    #[subscribe_message("sign")]
    #[public]
    #[use_guards(AuthorGuard)]
    async fn sign(&self, text: String, author: Ambient<Author>) -> String {
        format!("{text}, {}", author.into_inner().0)
    }

    #[subscribe_message("unwired")]
    #[public]
    async fn unwired(&self, missing: Ambient<Unattached>) -> String {
        let Unattached = missing.into_inner();
        "attached".to_owned()
    }
}

#[module(imports = [WsModule], providers = [CallerGuard, AuthorGuard, CtxGateway])]
struct CtxModule;

async fn connect() -> (WsApp, WsSocket) {
    let app = TestApp::builder()
        .module::<CtxModule>()
        .build_ws()
        .await
        .expect("boots");
    let socket = app.socket("/ctx").header("x-name", "ada").connect().await;
    (app, socket)
}

#[tokio::test]
async fn a_message_reads_what_the_upgrades_guard_attached() {
    let (app, mut socket) = connect().await;
    socket.send("whoami", serde_json::Value::Null).await;
    assert_eq!(socket.next_envelope().await["data"], "ada");
    app.shutdown().await.expect("shuts down");
}

#[tokio::test]
async fn a_message_reads_what_its_own_guard_attached_beside_its_payload() {
    let (app, mut socket) = connect().await;
    socket.send("sign", serde_json::json!("hi")).await;
    assert_eq!(socket.next_envelope().await["data"], "hi, ada (author)");
    app.shutdown().await.expect("shuts down");
}

#[tokio::test]
async fn an_unattached_ctx_answers_opaquely_and_files_its_type() {
    let logs = LogCapture::install();
    let (app, mut socket) = connect().await;

    socket.send("unwired", serde_json::Value::Null).await;
    let envelope = socket.next_envelope().await;

    assert_eq!(
        envelope["data"]["error"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{envelope}"
    );
    assert_eq!(envelope["data"]["code"], "INTERNAL", "{envelope}");
    assert!(!envelope.to_string().contains("Unattached"), "{envelope}");
    let line = logs.expect_one(nest_rs_ws::TARGET, "websocket message failed");
    assert!(
        line.field("error")
            .is_some_and(|error| error.contains("Unattached")),
        "{:?}",
        line.fields,
    );
    app.shutdown().await.expect("shuts down");
}
