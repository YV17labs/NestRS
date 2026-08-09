//! `src/registry.rs` + `src/composite.rs`: several `#[mcp]` hosts, one endpoint.
//!
//! The MCP spec namespaces tools **per endpoint** and every shipped client
//! config points at a single URL, so a product exposing several domains over
//! MCP needs all their tools on one path. That is what makes the merge a
//! framework concern rather than a convenience: without it a product has to
//! fold every domain into one god-host, inverting the one-adapter-per-feature
//! layout the rules mandate.
//!
//! This is the **composition witness** `CLAUDE.md` § *Shipping a new
//! capability* step 5 requires: two modules, each with its own `#[mcp]`
//! provider on one path, booted through `TestApp` and driven over the real
//! streamable-HTTP endpoint.

use std::sync::Arc;

use nest_rs_core::{DiscoveryService, module};
use nest_rs_http::HttpEndpointMeta;
use nest_rs_mcp::model::{
    CallToolRequestParams, CallToolResponse, GetPromptResult, Implementation, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, PromptMessage, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, Role, ServerCapabilities,
    ServerInfo, Tool,
};
use nest_rs_mcp::rmcp;
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::service::{RequestContext, RoleServer};
use nest_rs_mcp::{
    AllowAllMcpGuard, CallToolResult, ContentBlock, McpEndpoint, McpError, McpModule,
    McpOperationGuard, McpOptions, McpSetup, ServerHandler, hosts_on, mcp, prompt, prompt_handler,
    prompt_router, tool, tool_handler, tool_router,
};
use nest_rs_testing::mcp::{call_method, call_tool, initialize, open_session, result};
use nest_rs_testing::{LogCapture, TestApp};

/// The shared endpoint both `audio`-shaped and `posts`-shaped hosts below mount
/// on — one path, the way a client config points at one URL.
const SHARED: &str = "/mcp";

// --- two features, one endpoint --------------------------------------------

/// Stands in for a feature that serves tools only.
#[mcp(path = "/mcp")]
#[derive(Clone)]
struct AlphaTool;

#[tool_router]
impl AlphaTool {
    #[tool(description = "Alpha's own tool.")]
    async fn alpha_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            "alpha here",
        )]))
    }
}

#[tool_handler]
impl ServerHandler for AlphaTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Alpha instructions.")
    }
}

/// Stands in for a feature that serves all three capabilities, so the merge is
/// exercised beyond `tools/call`.
#[mcp(path = "/mcp")]
#[derive(Clone)]
struct BetaTool;

#[tool_router]
impl BetaTool {
    #[tool(description = "Beta's own tool.")]
    async fn beta_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            "beta here",
        )]))
    }
}

#[prompt_router]
impl BetaTool {
    #[prompt(description = "Beta's own prompt.")]
    async fn beta_prompt(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "beta prompt body",
        )]))
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for BetaTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
        .with_instructions("Beta instructions.")
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![Resource::new("beta://one", "Beta one")],
            ..ListResourcesResult::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri != "beta://one" {
            return Err(McpError::resource_not_found("unknown resource", None));
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::text("beta body", request.uri)]).into())
    }
}

/// Each host lives in its own module — the adapter shape the rules mandate,
/// which is exactly what the merge exists to keep possible.
#[module(providers = [AlphaTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct AlphaMcpModule;

#[module(providers = [BetaTool])]
struct BetaMcpModule;

#[module(imports = [AlphaMcpModule, BetaMcpModule])]
struct SharedEndpointApp;

async fn shared_app() -> TestApp {
    TestApp::for_module::<SharedEndpointApp>()
        .await
        .expect("two #[mcp] hosts on one path boot")
}

#[tokio::test]
async fn two_modules_on_one_path_share_a_single_mount() {
    let app = shared_app().await;

    let mounts = DiscoveryService::new(app.container())
        .meta::<HttpEndpointMeta>()
        .into_iter()
        .filter(|discovered| discovered.meta.label() == "mcp")
        .count();
    assert_eq!(
        mounts, 1,
        "two hosts on one path attach exactly one endpoint — a second would be \
         the transport's duplicate-mount boot error",
    );

    let hosts = hosts_on(app.container(), SHARED);
    let names: Vec<&str> = hosts.iter().map(|meta| meta.host()).collect();
    assert_eq!(names, ["AlphaTool", "BetaTool"]);
}

#[tokio::test]
async fn both_tool_sets_answer_on_one_endpoint() {
    let app = shared_app().await;

    let alpha = call_tool(app.http(), SHARED, "alpha_ping", None).await;
    assert!(
        alpha.contains("alpha here"),
        "the first module's tool answers on the shared path: {alpha}",
    );

    let beta = call_tool(app.http(), SHARED, "beta_ping", None).await;
    assert!(
        beta.contains("beta here"),
        "the second module's tool answers on the *same* path — this is the whole \
         point of the chantier: {beta}",
    );
}

#[tokio::test]
async fn tools_list_is_the_union_of_every_host() {
    let app = shared_app().await;
    let session = open_session(app.http(), SHARED, None).await;

    let body = call_method(app.http(), SHARED, &session, None, "tools/list", json!({})).await;
    let names: Vec<String> = result(&body)["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
        .collect();

    assert!(
        names.contains(&"alpha_ping".to_owned()) && names.contains(&"beta_ping".to_owned()),
        "a client discovers both features' tools in one listing: {names:?}",
    );
}

/// A merged endpoint is not tools-only: prompts and resources contributed by one
/// host stay reachable beside another host's tools, and the capability
/// declaration advertises the union — a client that reads `tools` alone would
/// never ask for the rest.
#[tokio::test]
async fn prompts_and_resources_survive_the_merge() {
    let app = shared_app().await;
    let session = open_session(app.http(), SHARED, None).await;

    let prompts = call_method(
        app.http(),
        SHARED,
        &session,
        None,
        "prompts/list",
        json!({}),
    )
    .await;
    assert!(
        prompts.contains("beta_prompt"),
        "the prompt host's prompts are listed on the shared endpoint: {prompts}",
    );

    let resources = call_method(
        app.http(),
        SHARED,
        &session,
        None,
        "resources/read",
        json!({ "uri": "beta://one" }),
    )
    .await;
    assert!(
        resources.contains("beta body"),
        "a resource read is routed to the host that owns the URI: {resources}",
    );
}

#[tokio::test]
async fn initialize_advertises_the_union_of_capabilities_and_instructions() {
    let app = shared_app().await;
    let body = initialize(app.http(), SHARED, None).await;

    let advertised = &result(&body)["result"];
    assert!(
        advertised["capabilities"]["tools"].is_object()
            && advertised["capabilities"]["prompts"].is_object()
            && advertised["capabilities"]["resources"].is_object(),
        "a capability any host serves is a capability the endpoint serves: {advertised}",
    );

    let instructions = advertised["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.contains("Alpha instructions.") && instructions.contains("Beta instructions."),
        "both hosts' instructions reach the one blurb a client reads: {instructions:?}",
    );
}

// --- the endpoint's own identity ---------------------------------------------
//
// An MCP endpoint is one server to every client that reaches it: the protocol
// carries one `serverInfo` and one `instructions`, on `initialize` and on
// `server/discover` alike — the same shape `new McpServer({name, version})`
// builds in the TypeScript SDK and a FastMCP parent keeps when it mounts
// children. A `#[mcp]` host owns a *feature*, so on a shared path none of them
// can speak for the endpoint; the app declares it.

/// The identity `SharedEndpointApp` above deliberately leaves undeclared, so
/// both shapes are witnessed against the same two hosts.
fn declared_identity() -> McpEndpoint {
    McpEndpoint::new(SHARED, "composition-witness", "9.9.9")
        .title("Composition witness")
        .instructions("Endpoint instructions.")
}

/// The import site every declaration test below uses: identity only, server
/// options left to the environment.
fn declare(endpoint: McpEndpoint) -> McpSetup {
    McpModule::for_root(McpOptions {
        endpoints: vec![endpoint],
        ..Default::default()
    })
}

#[module(imports = [AlphaMcpModule, BetaMcpModule, declare(declared_identity())])]
struct DeclaredEndpointApp;

#[tokio::test]
async fn a_declared_identity_is_what_the_endpoint_reports() {
    let app = TestApp::for_module::<DeclaredEndpointApp>()
        .await
        .expect("a declared endpoint boots");

    let body = initialize(app.http(), SHARED, None).await;
    let advertised = &result(&body)["result"];

    assert_eq!(
        advertised["serverInfo"]["name"].as_str(),
        Some("composition-witness"),
        "the endpoint reports the app's name, not whichever host `imports` listed \
         first: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["version"].as_str(),
        Some("9.9.9"),
        "…at the app's own version: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["title"].as_str(),
        Some("Composition witness"),
    );

    assert_eq!(
        advertised["instructions"].as_str(),
        Some("Endpoint instructions."),
        "declared instructions *replace* the hosts', rather than being appended \
         to a blurb no one wrote: {advertised}",
    );

    assert!(
        advertised["capabilities"]["tools"].is_object()
            && advertised["capabilities"]["prompts"].is_object()
            && advertised["capabilities"]["resources"].is_object(),
        "identity is declared, capabilities stay observed — a declaration cannot \
         claim a surface no host serves, nor hide one: {advertised}",
    );
}

/// Every operation still routes across both hosts: naming the endpoint is a
/// declaration, not a takeover.
#[tokio::test]
async fn declaring_an_identity_changes_nothing_about_routing() {
    let app = TestApp::for_module::<DeclaredEndpointApp>()
        .await
        .expect("a declared endpoint boots");

    let alpha = call_tool(app.http(), SHARED, "alpha_ping", None).await;
    let beta = call_tool(app.http(), SHARED, "beta_ping", None).await;
    assert!(
        alpha.contains("alpha here") && beta.contains("beta here"),
        "both hosts still answer under the declared endpoint: {alpha} / {beta}",
    );
}

/// The fallback is order-dependent by construction — `imports = [..]` decides
/// whose name a client sees — so a shared endpoint that takes it is reported,
/// with the remedy in the event. A lone host *is* its endpoint, so the ordinary
/// single-host mount stays silent.
/// A host that names itself is a whole server on its own path — the shape every
/// MCP SDK builds — so the framework has nothing to say about it.
#[mcp(path = "/named")]
#[derive(Clone)]
struct SelfNamedTool;

#[tool_router]
impl SelfNamedTool {
    #[tool(description = "The only tool of a self-contained server.")]
    async fn named_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("named")]))
    }
}

#[tool_handler]
impl ServerHandler for SelfNamedTool {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("standalone-host", "1.2.3");
        info
    }
}

#[module(providers = [SelfNamedTool])]
struct SelfNamedModule;

#[tokio::test]
async fn a_lone_host_that_names_itself_needs_no_declaration() {
    let logs = LogCapture::install();
    let _app = TestApp::for_module::<SelfNamedModule>()
        .await
        .expect("one self-named host boots");

    assert!(
        logs.find("nest_rs::mcp", UNDECLARED_IDENTITY).is_empty()
            && logs.find("nest_rs::mcp", SDK_DEFAULT_IDENTITY).is_empty(),
        "a lone host that named itself is a complete endpoint: {:#?}",
        logs.events(),
    );
}

/// rmcp's `ServerInfo::new` leaves `serverInfo` at the **SDK's** build identity,
/// so a host that never names itself makes its endpoint introduce itself to
/// every client as `rmcp`, at rmcp's version. Nothing fails, which is exactly
/// why it has to be said out loud at boot.
#[tokio::test]
async fn an_endpoint_nobody_named_reports_the_sdk_and_is_told_so() {
    let logs = LogCapture::install();
    let _app = TestApp::for_module::<AlphaOnlyApp>()
        .await
        .expect("one host boots");

    let event = logs.expect_one("nest_rs::mcp", SDK_DEFAULT_IDENTITY);
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("host").as_deref(), Some("AlphaTool"));
    assert_eq!(
        event.field("reports_as"),
        Some(
            ServerInfo::new(ServerCapabilities::default())
                .server_info
                .name
        ),
        "the event names what the client actually reads — taken from the SDK's \
         own constructor, so it cannot drift from the rmcp the framework built \
         against",
    );
}

/// The message the check above carries.
const SDK_DEFAULT_IDENTITY: &str = "an MCP endpoint introduces itself with the SDK's own name and \
     version — neither the host nor the app named it";

#[tokio::test]
async fn an_undeclared_shared_endpoint_is_reported_at_boot() {
    let logs = LogCapture::install();
    let _app = shared_app().await;

    let event = logs.expect_one("nest_rs::mcp", UNDECLARED_IDENTITY);
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reports_as").as_deref(),
        Some("AlphaTool"),
        "the event names the host whose identity the endpoint borrowed",
    );
}

/// The message the warn above carries, kept next to both assertions so they
/// cannot drift from each other.
const UNDECLARED_IDENTITY: &str = "several MCP hosts share an endpoint whose identity nobody declared — it reports the first \
     host's";

#[module(imports = [
    AlphaMcpModule,
    declare(McpEndpoint::new("/typo", "orphan", "1.0.0")),
])]
struct OrphanDeclarationApp;

/// A declaration is a statement about a real endpoint. Pointed at a path no
/// host serves it would otherwise do nothing at all — the one answer a
/// declaration must never silently get.
#[tokio::test]
async fn a_declaration_no_host_serves_fails_boot() {
    let err = match TestApp::for_module::<OrphanDeclarationApp>().await {
        Ok(_) => panic!("an identity declared for a path nobody serves must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("\"/typo\""),
        "the failure names the path that reaches nothing: {err}",
    );
}

#[module(imports = [
    AlphaMcpModule,
    BetaMcpModule,
    declare(McpEndpoint::new(SHARED, "first-name", "1.0.0")),
    declare(McpEndpoint::new(SHARED, "second-name", "1.0.0")),
])]
struct ContestedDeclarationApp;

/// One endpoint reports one `serverInfo`, so two declarations that disagree are
/// a question the framework cannot answer — and picking one silently would put
/// the endpoint's name back where declaring it was meant to take it from:
/// import order.
#[tokio::test]
async fn two_disagreeing_declarations_for_one_path_fail_boot() {
    let err = match TestApp::for_module::<ContestedDeclarationApp>().await {
        Ok(_) => panic!("two identities for one endpoint must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("first-name") && err.contains("second-name"),
        "the failure names both claimants: {err}",
    );
}

// --- a host that declares no router ------------------------------------------

/// A host that hand-writes `call_tool`/`list_tools` instead of using
/// `#[tool_router]`. `#[mcp]` must sit on it unchanged: the expansion asks
/// `<Self>::tool_router()` for the statically-known names, and with no inherent
/// one the empty `DefaultToolRouter` fallback answers. Its tools are still
/// served — the merge offers an unclaimed name to each host in turn — they are
/// simply not *statically* known.
#[mcp(path = "/manual")]
#[derive(Clone)]
struct ManualTool;

impl ServerHandler for ManualTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: vec![Tool::new(
                "manual_ping",
                "Hand-routed.",
                Arc::new(rmcp::model::JsonObject::new()),
            )],
            ..ListToolsResult::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if request.name != "manual_ping" {
            return Err(McpError::invalid_params("tool not found", None));
        }
        Ok(CallToolResult::success(vec![ContentBlock::text("manual here")]).into())
    }
}

#[mcp(path = "/manual")]
#[derive(Clone)]
struct RoutedTool;

#[tool_router]
impl RoutedTool {
    #[tool(description = "A router-declared peer of the hand-written host.")]
    async fn routed_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("routed")]))
    }
}

#[tool_handler]
impl ServerHandler for RoutedTool {}

#[module(providers = [ManualTool, RoutedTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct MixedHostModule;

/// The gap this pins is the one the merge could hide. `#[mcp]` reads a host's
/// tool names through rmcp's default `tool_router()`; a host that keeps its
/// router under another name — rmcp's own answer for a host with many tools —
/// declares none, so `check_duplicate_tools` has no candidate names for it and
/// a clash between two such hosts would go unreported. The framework does not
/// get to fail silently there: it says so at boot, naming the hosts and the
/// remedy.
#[tokio::test]
async fn a_host_the_boot_check_cannot_read_is_reported_at_boot() {
    let logs = LogCapture::install();
    let _app = TestApp::for_module::<MixedHostModule>()
        .await
        .expect("the path still boots — this is a gap, not a refusal");

    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp hosts on a shared path declare no tool names the boot check can read",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("hosts").as_deref(),
        Some("ManualTool"),
        "the host whose tools are invisible is named, and its router-backed peer is not",
    );
}

#[tokio::test]
async fn a_host_without_a_tool_router_still_serves_beside_one_that_has() {
    let app = TestApp::for_module::<MixedHostModule>()
        .await
        .expect("a hand-written host and a router-backed one share a path");

    let manual = call_tool(app.http(), "/manual", "manual_ping", None).await;
    assert!(
        manual.contains("manual here"),
        "a name no host claims through `get_tool` is offered to each in turn: {manual}",
    );

    let routed = call_tool(app.http(), "/manual", "routed_ping", None).await;
    assert!(
        routed.contains("routed"),
        "…and the router-backed peer is still routed by name: {routed}",
    );
}

// --- distinct paths stay distinct -------------------------------------------

#[mcp(path = "/other/mcp")]
#[derive(Clone)]
struct OtherTool;

#[tool_router]
impl OtherTool {
    #[tool(description = "A tool on its own endpoint.")]
    async fn other_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("other")]))
    }
}

#[tool_handler]
impl ServerHandler for OtherTool {}

#[module(providers = [OtherTool])]
struct OtherMcpModule;

#[module(imports = [AlphaMcpModule, BetaMcpModule, OtherMcpModule])]
struct TwoPathApp;

/// Grouping is **by path**. An app that deliberately serves a second endpoint
/// keeps it separate — merging every `#[mcp]` host in the process would destroy
/// the per-endpoint namespacing the spec gives, and `demo` mounts two paths for
/// exactly that reason.
#[tokio::test]
async fn a_second_path_is_a_second_endpoint() {
    let app = TestApp::for_module::<TwoPathApp>()
        .await
        .expect("two paths boot");

    assert_eq!(
        hosts_on(app.container(), SHARED).len(),
        2,
        "the shared path still carries both of its hosts",
    );
    assert_eq!(
        hosts_on(app.container(), "/other/mcp").len(),
        1,
        "the second path carries only its own",
    );

    let session = open_session(app.http(), "/other/mcp", None).await;
    let body = call_method(
        app.http(),
        "/other/mcp",
        &session,
        None,
        "tools/list",
        json!({}),
    )
    .await;
    assert!(
        body.contains("other_ping") && !body.contains("alpha_ping"),
        "the second endpoint lists its own tools and nobody else's: {body}",
    );
}

// --- module gating -----------------------------------------------------------

/// Only `AlphaMcpModule` is imported, so `BetaTool` is linked into this binary
/// but never registered. Per-app subsets are the whole point of module gating,
/// and here it is structural: metadata is attached from `register`, which never
/// runs for a provider no imported module owns.
#[module(imports = [AlphaMcpModule])]
struct AlphaOnlyApp;

#[tokio::test]
async fn an_unimported_host_contributes_nothing() {
    let app = TestApp::for_module::<AlphaOnlyApp>()
        .await
        .expect("one host boots");

    let names: Vec<&str> = hosts_on(app.container(), SHARED)
        .iter()
        .map(|meta| meta.host())
        .collect();
    assert_eq!(names, ["AlphaTool"]);

    let session = open_session(app.http(), SHARED, None).await;
    let body = call_method(app.http(), SHARED, &session, None, "tools/list", json!({})).await;
    assert!(
        !body.contains("beta_ping"),
        "a host whose module the app does not import stays inert: {body}",
    );
}

// --- the one new failure mode the merge introduces ----------------------------

#[mcp(path = "/clash")]
#[derive(Clone)]
struct FirstClashingTool;

#[tool_router]
impl FirstClashingTool {
    #[tool(description = "First owner of the name.")]
    async fn shared_name(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("first")]))
    }
}

#[tool_handler]
impl ServerHandler for FirstClashingTool {}

#[mcp(path = "/clash")]
#[derive(Clone)]
struct SecondClashingTool;

#[tool_router]
impl SecondClashingTool {
    #[tool(description = "Second owner of the same name.")]
    async fn shared_name(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("second")]))
    }
}

#[tool_handler]
impl ServerHandler for SecondClashingTool {}

#[module(providers = [FirstClashingTool, SecondClashingTool])]
struct ClashingModule;

/// MCP addresses a tool by bare name within an endpoint, so two hosts claiming
/// one name make the loser unreachable — and *which* one loses depends on
/// registration order. That is a boot error naming both, never a runtime
/// surprise. The invariant could not even be expressed before the merge.
#[tokio::test]
async fn a_duplicate_tool_name_on_one_path_fails_boot() {
    let err = match TestApp::for_module::<ClashingModule>().await {
        Ok(_) => panic!("two hosts claiming one tool name must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("shared_name"),
        "the failure names the contested tool: {err}",
    );
    assert!(
        err.contains("FirstClashingTool") && err.contains("SecondClashingTool"),
        "…and both owning providers: {err}",
    );
}
