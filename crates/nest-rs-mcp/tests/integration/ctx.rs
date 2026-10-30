//! `src/ctx.rs`: a `Ctx<T>` parameter reads what a guard attached — the `/mcp`
//! request's guards for every operation, an operation's own for its body — on
//! a tool and a resource alike, and is no argument of the protocol; absent,
//! the operation answers opaquely and files the type.

use nest_rs_core::{AmbientValue, Ctx, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpView};
use nest_rs_mcp::{McpError, McpView, mcp, tools};
use nest_rs_testing::mcp::{call_method_with, open_session_with, result};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

const PATH: &str = "/mcp/ctx";

/// Who the request's guard says is calling.
#[derive(Clone)]
struct Caller(String);

impl AmbientValue for Caller {}

/// What an operation's own guard derives from the caller.
#[derive(Clone)]
struct Author(String);

impl AmbientValue for Author {}

/// What no guard attaches.
#[derive(Clone)]
struct Unattached;

impl AmbientValue for Unattached {}

/// Attaches the caller the `/mcp` request names, for every operation it
/// carries, named its actor as the authentication guard names it, so a session
/// opens for it.
#[injectable]
#[derive(Default, Layer)]
struct CallerGuard;

#[async_trait]
impl Guard for CallerGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let name = HttpView::of(cx)?
            .request()
            .header("x-name")
            .unwrap_or("anonymous");
        nest_rs_core::__private::set_actor_id(name);
        let caller = Caller(name.to_owned());
        cx.attach(caller);
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for CallerGuard {}

/// Derives the author from the caller, at the operation it is bound on.
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

impl GuardFor<McpView> for AuthorGuard {}

#[mcp(path = "/mcp/ctx")]
#[derive(Clone, Default)]
struct CtxTools;

#[tools]
impl CtxTools {
    /// Say who is calling.
    #[tool]
    #[public]
    async fn whoami(&self, caller: Ctx<Caller>) -> Result<String, McpError> {
        Ok(caller.into_inner().0)
    }

    /// Sign as the author the operation's guard derived.
    #[tool]
    #[public]
    #[use_guards(AuthorGuard)]
    async fn sign(&self, author: Ctx<Author>) -> Result<String, McpError> {
        Ok(author.into_inner().0)
    }

    /// Read what nothing attached.
    #[tool]
    #[public]
    async fn unwired(&self, missing: Ctx<Unattached>) -> Result<String, McpError> {
        let Unattached = missing.into_inner();
        Ok("attached".to_owned())
    }

    /// A note addressed to its reader.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: String, caller: Ctx<Caller>) -> Result<String, McpError> {
        Ok(format!("note {id} for {}", caller.into_inner().0))
    }
}

#[module(providers = [
    CallerGuard,
    AuthorGuard,
    CtxTools,
    EndpointGuards<McpView, (CallerGuard,)>,
])]
struct CtxModule;

async fn boot() -> TestApp {
    TestApp::for_module::<CtxModule>().await.expect("boots")
}

/// What `method` with `params` answers on a session `ada` opened.
async fn ask(app: &TestApp, method: &str, params: Value) -> Value {
    let named = [("x-name", "ada")];
    let session = open_session_with(app.http(), PATH, None, &named, json!({})).await;
    let body = call_method_with(app.http(), PATH, &session, None, &named, method, params).await;
    result(&body)
}

async fn call(app: &TestApp, tool: &str) -> Value {
    ask(app, "tools/call", json!({ "name": tool, "arguments": {} })).await
}

#[tokio::test]
async fn a_tool_reads_what_the_requests_guard_attached() {
    let app = boot().await;
    assert_eq!(
        call(&app, "whoami").await["result"]["content"][0]["text"],
        "ada"
    );
}

#[tokio::test]
async fn a_tool_reads_what_its_own_guard_attached() {
    let app = boot().await;
    assert_eq!(
        call(&app, "sign").await["result"]["content"][0]["text"],
        "ada (author)"
    );
}

#[tokio::test]
async fn a_resource_reads_what_the_requests_guard_attached() {
    let app = boot().await;
    let read = ask(&app, "resources/read", json!({ "uri": "note://7" })).await;
    assert_eq!(
        read["result"]["contents"][0]["text"], "note 7 for ada",
        "{read}"
    );
}

#[tokio::test]
async fn a_ctx_is_no_argument_of_the_tool() {
    let app = boot().await;
    let listed = ask(&app, "tools/list", json!({})).await;
    let whoami = listed["result"]["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "whoami"))
        .unwrap_or_else(|| panic!("whoami is listed: {listed}"));
    assert_eq!(
        whoami["inputSchema"]["properties"]
            .as_object()
            .map_or(0, |p| p.len()),
        0,
        "{whoami}"
    );
}

#[tokio::test]
async fn an_unattached_ctx_answers_opaquely_and_files_its_type() {
    let logs = LogCapture::install();
    let app = boot().await;

    let answered = call(&app, "unwired").await;

    assert_eq!(
        answered["error"]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{answered}"
    );
    assert!(!answered.to_string().contains("Unattached"), "{answered}");
    let line = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed");
    assert!(
        line.field("error")
            .is_some_and(|error| error.contains("Unattached")),
        "{:?}",
        line.fields,
    );
}
