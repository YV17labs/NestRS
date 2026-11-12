//! `src/ambient.rs`: what the `/mcp` request carries — its trace, what its
//! guards attached, what a layer around it made ambient — reaches each
//! operation on the task rmcp runs it on, in session and stateless modes,
//! and an `Ambient<T>` parameter reads it.

use std::sync::Arc;

use nest_rs_core::{
    AmbientValue, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable,
    interceptor, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpView};
use nest_rs_mcp::{McpError, McpView, mcp, tools};
use nest_rs_testing::mcp::{open_session, post_message_with, result};
use nest_rs_testing::{TestApp, TestResponse};
use serde_json::json;

const PATH: &str = "/mcp/ambient";

/// What the request's guard says the caller is.
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// What a layer around the `/mcp` request makes ambient for the rest of it.
struct Tenant(&'static str);

impl AmbientValue for Tenant {}

/// Attaches a [`Caller`] to every request, named its actor as the
/// authentication guard names it, so a session opens for it.
#[injectable]
#[derive(Default, Layer)]
struct Attaching;

#[async_trait]
impl Guard for Attaching {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("ada");
        cx.attach(Caller("ada"));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for Attaching {}

/// Around every HTTP unit, the `/mcp` request among them: runs the rest of it
/// with a [`Tenant`] ambient.
#[interceptor(stage = unit)]
struct Tenancy;

impl Layer for Tenancy {
    fn serves(&self, unit: nest_rs_core::operation_log::Unit) -> bool {
        unit.edge() == Some(nest_rs_core::Edge::Http)
    }
}

#[async_trait]
impl Interceptor for Tenancy {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        nest_rs_core::with_ambient(Arc::new(Tenant("acme")), next.run(cx)).await
    }
}

#[mcp(path = "/mcp/ambient")]
#[derive(Clone, Default)]
struct WhoamiTools;

#[tools]
impl WhoamiTools {
    /// Say what the operation runs under: its trace, its caller, its tenant.
    #[tool]
    #[public]
    async fn whoami(&self) -> Result<String, McpError> {
        let trace =
            nest_rs_core::current_trace_id().map_or_else(|| "none".to_owned(), |id| id.to_hex());
        let caller = nest_rs_core::ambient::<Caller>().map_or("nobody", |caller| caller.0);
        let tenant = nest_rs_core::ambient::<Tenant>().map_or("no tenant", |tenant| tenant.0);
        Ok(format!("{trace} {caller} {tenant}"))
    }
}

#[module(providers = [
    WhoamiTools,
    Attaching,
    Tenancy,
    EndpointGuards<McpView, (Attaching,)>,
])]
struct AmbientModule;

/// What the operation answered it ran under, beside the trace the request
/// carrying it ran in, as its `traceresponse` reports it.
async fn said(response: TestResponse) -> (String, String) {
    let trace = response
        .0
        .headers()
        .get("traceresponse")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('-').nth(1))
        .unwrap_or_else(|| panic!("the request reports its trace"))
        .to_owned();
    let body = response
        .0
        .into_body()
        .into_string()
        .await
        .expect("a response body");
    let said = result(&body)["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the tool answered: {body}"))
        .to_owned();
    (said, trace)
}

#[tokio::test]
async fn a_session_operation_runs_under_its_requests_trace_caller_and_ambient() {
    let app = TestApp::for_module::<AmbientModule>().await.expect("boots");
    let session = open_session(app.http(), PATH, None).await;

    let response = post_message_with(
        app.http(),
        PATH,
        Some(&session),
        None,
        &[],
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "whoami", "arguments": {} },
        }),
    )
    .await;

    let (said, trace) = said(response).await;
    assert_eq!(said, format!("{trace} ada acme"));
}

#[tokio::test]
async fn a_stateless_operation_runs_under_its_requests_trace_caller_and_ambient() {
    let app = TestApp::for_module::<AmbientModule>().await.expect("boots");
    let version = nest_rs_mcp::model::ProtocolVersion::LATEST;

    let response = post_message_with(
        app.http(),
        PATH,
        None,
        None,
        &[
            ("mcp-protocol-version", version.as_str()),
            ("mcp-method", "tools/call"),
            ("mcp-name", "whoami"),
        ],
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "whoami",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": version.as_str(),
                    "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-mcp", "version": "0" },
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }),
    )
    .await;

    let (said, trace) = said(response).await;
    assert_eq!(said, format!("{trace} ada acme"));
}

mod parameter {
    //! An `Ambient<T>` parameter reads what a guard attached — the `/mcp`
    //! request's guards for every operation, an operation's own for its body — on
    //! a tool and a resource alike, and is no argument of the protocol; absent,
    //! the operation answers opaquely and files the type.

    use nest_rs_core::{
        Ambient, AmbientValue, Layer, UnitContext, async_trait, injectable, module,
    };
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
        async fn whoami(&self, caller: Ambient<Caller>) -> Result<String, McpError> {
            Ok(caller.into_inner().0)
        }

        /// Sign as the author the operation's guard derived.
        #[tool]
        #[public]
        #[use_guards(AuthorGuard)]
        async fn sign(&self, author: Ambient<Author>) -> Result<String, McpError> {
            Ok(author.into_inner().0)
        }

        /// Read what nothing attached.
        #[tool]
        #[public]
        async fn unwired(&self, missing: Ambient<Unattached>) -> Result<String, McpError> {
            let Unattached = missing.into_inner();
            Ok("attached".to_owned())
        }

        /// A note addressed to its reader.
        #[resource(template = "note://{id}")]
        #[public]
        async fn note(&self, id: String, caller: Ambient<Caller>) -> Result<String, McpError> {
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
}

mod needed_principal {
    //! A `#[public]` operation reading as an `Ambient<P>` the principal a guard of
    //! its chain produces: a caller that guard admitted without one answers
    //! `UNAUTHENTICATED`, filed as that guard's denial, never the wiring error an
    //! unattached value is.

    use nest_rs_core::{
        Ambient, AmbientValue, Layer, UnitContext, async_trait, code, injectable, module,
    };
    use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};
    use nest_rs_http::{EndpointGuards, HttpView};
    use nest_rs_mcp::{McpError, McpView, mcp, tools};
    use nest_rs_testing::mcp::{call_method_with, open_session_with, result};
    use nest_rs_testing::{LogCapture, TestApp};
    use serde_json::{Value, json};

    const PATH: &str = "/mcp/named";

    /// Who the request says is calling, whether or not it named itself.
    #[derive(Clone)]
    struct Caller(String);

    impl AmbientValue for Caller {}

    /// Who the caller's credential names.
    #[derive(Clone)]
    struct Named(String);

    impl AmbientValue for Named {}

    /// Attaches the caller the `/mcp` request names, `anonymous` when it names
    /// none, and names its actor so a session opens for it.
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

    /// Identifies a caller the request named and admits one it did not, as a
    /// guard reading a credential the operation carries would.
    #[injectable]
    #[derive(Default, Layer)]
    struct Identifies;

    #[async_trait]
    impl Guard for Identifies {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }

        fn produced_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<Named>())
        }

        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let caller = cx.require::<Caller>()?;
            if caller.0 != "anonymous" {
                let named = Named(caller.0.clone());
                cx.attach(named);
            }
            Ok(())
        }
    }

    impl GuardFor<McpView> for Identifies {}

    #[mcp(path = "/mcp/named")]
    #[derive(Clone, Default)]
    struct NamedTools;

    #[tools]
    impl NamedTools {
        /// Say who the credential names.
        #[tool]
        #[public]
        #[use_guards(Identifies)]
        async fn named(&self, named: Ambient<Named>) -> Result<String, McpError> {
            Ok(named.into_inner().0)
        }
    }

    #[module(providers = [
        CallerGuard,
        Identifies,
        NamedTools,
        EndpointGuards<McpView, (CallerGuard,)>,
    ])]
    struct NamedModule;

    /// What calling `named` answers, the request carrying `headers`.
    async fn call(app: &TestApp, headers: &[(&str, &str)]) -> Value {
        let session = open_session_with(app.http(), PATH, None, headers, json!({})).await;
        let params = json!({ "name": "named", "arguments": {} });
        let body = call_method_with(
            app.http(),
            PATH,
            &session,
            None,
            headers,
            "tools/call",
            params,
        )
        .await;
        result(&body)
    }

    #[tokio::test]
    async fn a_public_operation_reading_its_principal_refuses_an_unidentified_caller() {
        let app = TestApp::for_module::<NamedModule>().await.expect("boots");
        let read = call(&app, &[("x-name", "ada")]).await;
        assert_eq!(read["result"]["content"][0]["text"], "ada", "{read}");

        let logs = LogCapture::install();
        let anonymous = call(&app, &[]).await;
        assert_eq!(
            anonymous["error"]["data"]["code"],
            code::UNAUTHENTICATED.as_str(),
            "{anonymous}"
        );
        let denial = logs.expect_one("nest_rs::layers", "guard denied the operation");
        assert_eq!(denial.level, "warn");
        assert_eq!(denial.field("guard").as_deref(), Some("Identifies"));
        assert!(
            logs.find(nest_rs_mcp::TARGET, "mcp operation failed")
                .is_empty(),
            "an unidentified caller is no wiring error",
        );
    }
}
