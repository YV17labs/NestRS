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
    ListToolsResult, PaginatedRequestParams, PromptMessage, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerInfo, Tool,
};
use nest_rs_mcp::rmcp;
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::service::{RequestContext, RoleServer};
use nest_rs_mcp::{
    AllowAllMcpGuard, CallToolResult, ContentBlock, DEFAULT_PATH, McpError, McpIdentity, McpModule,
    McpOperationGuard, McpOptions, McpSetup, ServerHandler, hosts_on, mcp, prompt, prompt_handler,
    prompt_router, tool, tool_handler, tool_router,
};
use nest_rs_testing::mcp::{call_method, call_tool, initialize, open_session, result};
use nest_rs_testing::{LogCapture, TestApp};

/// The endpoint both `audio`-shaped and `posts`-shaped hosts below mount on:
/// the prefix itself, which is where a bare `#[mcp]` lands and where a client
/// config points. Spelled out rather than read from `McpConfig` on purpose — a
/// test that computed it from the same source as the code would pass through a
/// change to the default.
const SHARED: &str = "/mcp";

// --- two features, one endpoint --------------------------------------------

/// Stands in for a feature that serves tools only.
#[mcp]
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
#[mcp]
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
// children. Two owners answer for it, and neither can shadow the other: the app
// names itself once, and at most one host on a path refines that for its own
// endpoint.

/// The app naming itself once — what every endpoint it exposes reports unless a
/// host on that endpoint says otherwise.
fn app_identity() -> McpSetup {
    McpModule::for_root(McpOptions {
        server: Some(
            McpIdentity::new("composition-witness", "9.9.9")
                .title("Composition witness")
                .instructions("Endpoint instructions."),
        ),
        ..Default::default()
    })
}

#[module(imports = [AlphaMcpModule, BetaMcpModule, app_identity()])]
struct DeclaredEndpointApp;

#[tokio::test]
async fn the_app_identity_is_what_a_shared_endpoint_reports() {
    let app = TestApp::for_module::<DeclaredEndpointApp>()
        .await
        .expect("a named app boots");

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
        "the app's instructions *replace* the hosts' own blurbs — on a shared \
         endpoint no host can see the whole, so nobody speaks for it but the \
         app: {advertised}",
    );

    assert!(
        advertised["capabilities"]["tools"].is_object()
            && advertised["capabilities"]["prompts"].is_object()
            && advertised["capabilities"]["resources"].is_object(),
        "identity is declared, capabilities stay observed — a declaration cannot \
         claim a surface no host serves, nor hide one: {advertised}",
    );
}

/// Every operation still routes across both hosts: naming the app is a
/// declaration, not a takeover.
#[tokio::test]
async fn declaring_an_identity_changes_nothing_about_routing() {
    let app = TestApp::for_module::<DeclaredEndpointApp>()
        .await
        .expect("a named app boots");

    let alpha = call_tool(app.http(), SHARED, "alpha_ping", None).await;
    let beta = call_tool(app.http(), SHARED, "beta_ping", None).await;
    assert!(
        alpha.contains("alpha here") && beta.contains("beta here"),
        "both hosts still answer under the declared endpoint: {alpha} / {beta}",
    );
}

// --- a host declaring for its own endpoint ------------------------------------

/// The path `OwnedTool` lands on — its own second endpoint, written whole.
const OWNED: &str = "/mcp/owned";

/// A feature whose endpoint stands apart. It renames the server — but
/// deliberately not the version, which is the binary's and which a shared
/// feature library cannot know, nor the instructions, which describe the server.
#[mcp(path = "/mcp/owned", name = "witness-owned")]
#[derive(Clone)]
struct OwnedTool;

#[tool_router]
impl OwnedTool {
    #[tool(description = "The tool of a self-framing endpoint.")]
    async fn owned_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("owned")]))
    }
}

#[tool_handler]
impl ServerHandler for OwnedTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Host blurb nobody should read.")
    }
}

#[module(providers = [OwnedTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct OwnedMcpModule;

#[module(imports = [OwnedMcpModule, app_identity()])]
struct OwnedEndpointApp;

/// The whole point of declaring at the host: a feature names the endpoint it
/// owns, in the file that serves it, and inherits everything it cannot know.
#[tokio::test]
async fn a_host_declares_its_own_endpoint_and_inherits_the_rest() {
    let app = TestApp::for_module::<OwnedEndpointApp>()
        .await
        .expect("a self-declaring host boots");

    let body = initialize(app.http(), OWNED, None).await;
    let advertised = &result(&body)["result"];

    assert_eq!(
        advertised["serverInfo"]["name"].as_str(),
        Some("witness-owned"),
        "the host's own name wins for its endpoint: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["version"].as_str(),
        Some("9.9.9"),
        "…while the version stays the app's, which is the only place that knows \
         it: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["title"].as_str(),
        Some("Composition witness"),
        "a field the host left out keeps the app's: {advertised}",
    );
    assert_eq!(
        advertised["instructions"].as_str(),
        Some("Endpoint instructions."),
        "…and the app's instructions reach it too, replacing the host's own \
         blurb rather than being appended to it: {advertised}",
    );
}

/// A host may name its endpoint in an app that named itself — but not in one
/// that did not, because the version would then be nobody's.
#[module(imports = [OwnedMcpModule])]
struct UnbackedNameApp;

#[tokio::test]
async fn a_name_with_no_version_behind_it_fails_boot() {
    let err = match TestApp::for_module::<UnbackedNameApp>().await {
        Ok(_) => panic!("a named endpoint with no version anywhere must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("OwnedTool") && err.contains("witness-owned"),
        "the failure names the host and what it called itself: {err}",
    );
    assert!(
        err.contains("McpIdentity::new"),
        "…and carries the remedy at the seam that fixes it: {err}",
    );
}

// --- two hosts declaring one endpoint ----------------------------------------

/// A peer of `OwnedTool` on the same path, also declaring. One endpoint reports
/// one `serverInfo`, so this is the ambiguity the framework refuses to resolve
/// silently.
#[mcp(path = "/mcp/owned", name = "witness-contender")]
#[derive(Clone)]
struct ContendingTool;

#[tool_router]
impl ContendingTool {
    #[tool(description = "A peer that also wants to name the endpoint.")]
    async fn contending_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            "contending",
        )]))
    }
}

#[tool_handler]
impl ServerHandler for ContendingTool {}

#[module(providers = [ContendingTool])]
struct ContendingMcpModule;

#[module(imports = [OwnedMcpModule, ContendingMcpModule, app_identity()])]
struct ContestedDeclarationApp;

/// Picking one silently would put the endpoint's name back where declaring it
/// was meant to take it from: import order.
#[tokio::test]
async fn two_hosts_declaring_one_endpoint_fail_boot() {
    let err = match TestApp::for_module::<ContestedDeclarationApp>().await {
        Ok(_) => panic!("two hosts naming one endpoint must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("OwnedTool") && err.contains("ContendingTool"),
        "the failure names both claimants: {err}",
    );
    assert!(err.contains(OWNED), "…and the endpoint they contest: {err}",);
}

// --- a declaration that reaches nothing ---------------------------------------

#[module(imports = [app_identity()])]
struct OrphanDeclarationApp;

/// A declaration is a statement about a real server. With no `#[mcp]` host in
/// the app it would otherwise do nothing at all — the one answer a declaration
/// must never silently get.
#[tokio::test]
async fn an_identity_no_host_serves_fails_boot() {
    let err = match TestApp::for_module::<OrphanDeclarationApp>().await {
        Ok(_) => panic!("an identity declared with no host at all must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("no #[mcp] host"),
        "the failure says the declaration reaches nothing: {err}",
    );
}

// --- two apps' worth of identity, and a path that means the same thing --------

/// A second, disagreeing `for_root`. The app identity carries no path, so no
/// per-path check can see this one: without its own check, `declared_server`
/// would take whichever the import graph reached first and the endpoint's name
/// would depend on `imports = [..]` order — the accident the seam removes.
#[module(imports = [
    AlphaMcpModule,
    app_identity(),
    McpModule::for_root(McpOptions {
        server: Some(McpIdentity::new("second-witness", "0.0.1")),
        ..Default::default()
    }),
])]
struct ContestedServerApp;

#[tokio::test]
async fn two_disagreeing_server_identities_fail_boot() {
    let err = match TestApp::for_module::<ContestedServerApp>().await {
        Ok(_) => panic!("two different server identities must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("composition-witness") && err.contains("second-witness"),
        "the failure names both claimants: {err}",
    );
}

/// Importing one `for_root` twice is not a conflict — `DynamicModule`
/// registration is deliberately not deduplicated, so the same declaration
/// arriving twice must stay silent.
#[module(imports = [AlphaMcpModule, app_identity(), app_identity()])]
struct RepeatedServerApp;

#[tokio::test]
async fn the_same_identity_declared_twice_is_not_a_conflict() {
    TestApp::for_module::<RepeatedServerApp>()
        .await
        .expect("one declaration imported twice boots");
}

/// `/mcp/` and `/mcp` are one endpoint to a client and two strings here. Left
/// alone they would each claim a mount, and poem's `Route::nest` panics on the
/// duplicate rather than reporting it — so the trailing slash is normalized
/// away and the host simply joins its peers.
#[mcp(path = "/mcp/")]
#[derive(Clone)]
struct TrailingSlashTool;

#[tool_router]
impl TrailingSlashTool {
    #[tool(description = "A tool whose path was written with a trailing slash.")]
    async fn trailing_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            "trailing",
        )]))
    }
}

#[tool_handler]
impl ServerHandler for TrailingSlashTool {}

#[module(providers = [TrailingSlashTool])]
struct TrailingSlashModule;

#[module(imports = [AlphaMcpModule, TrailingSlashModule])]
struct TrailingSlashApp;

#[tokio::test]
async fn a_trailing_slash_names_the_same_endpoint() {
    let app = TestApp::for_module::<TrailingSlashApp>()
        .await
        .expect("a path written with a trailing slash boots rather than panicking");

    let names: Vec<&str> = hosts_on(app.container(), SHARED)
        .iter()
        .map(|meta| meta.host())
        .collect();
    assert_eq!(
        names,
        ["AlphaTool", "TrailingSlashTool"],
        "the two spellings claim one mount, so the hosts merge",
    );

    let body = call_tool(app.http(), SHARED, "trailing_ping", None).await;
    assert!(
        body.contains("trailing"),
        "…and the endpoint serves both: {body}",
    );
}

// --- nobody naming it at all --------------------------------------------------

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

/// The two messages the check below carries — one per fact, because a single
/// message covering both would contradict its own `reports_as` field the moment
/// a host had named itself.
const SDK_DEFAULT_IDENTITY: &str = "an MCP endpoint introduces itself with the SDK's own name and \
     version — neither its hosts nor the app named it";
const UNDECLARED_IDENTITY: &str = "several MCP hosts share an endpoint whose identity nobody \
     declared — it reports the first host's";

/// rmcp's `ServerInfo::new` leaves `serverInfo` at the **SDK's** build identity,
/// so an endpoint nobody named introduces itself to every client as `rmcp`, at
/// rmcp's version. Nothing fails, which is exactly why it has to be said out
/// loud at boot.
#[tokio::test]
async fn an_endpoint_nobody_named_reports_the_sdk_and_is_told_so() {
    let logs = LogCapture::install();
    let _app = TestApp::for_module::<AlphaOnlyApp>()
        .await
        .expect("one host boots");

    let event = logs.expect_one("nest_rs::mcp", SDK_DEFAULT_IDENTITY);
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("hosts").as_deref(), Some("AlphaTool"));
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

#[tokio::test]
async fn an_undeclared_shared_endpoint_is_reported_at_boot() {
    let logs = LogCapture::install();
    let _app = shared_app().await;

    // Neither Alpha nor Beta names itself, so the SDK-default fact is the true
    // one here — and it is the only message emitted.
    let event = logs.expect_one("nest_rs::mcp", SDK_DEFAULT_IDENTITY);
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("hosts").as_deref(),
        Some("AlphaTool, BetaTool"),
        "the event names every host that could have spoken for the endpoint",
    );
}

/// The other branch, and the reason there are two messages: with hosts that
/// *did* name themselves, the endpoint answers with whichever registered first.
/// Reporting that as "introduces itself as the SDK" would be contradicted by the
/// event's own `reports_as` field.
#[mcp(path = "/mcp/self-named-peers")]
#[derive(Clone)]
struct FirstNamedTool;

#[tool_router]
impl FirstNamedTool {
    #[tool(description = "First of two self-named peers.")]
    async fn first_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("first")]))
    }
}

#[tool_handler]
impl ServerHandler for FirstNamedTool {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("first-peer", "1.0.0");
        info
    }
}

#[mcp(path = "/mcp/self-named-peers")]
#[derive(Clone)]
struct SecondNamedTool;

#[tool_router]
impl SecondNamedTool {
    #[tool(description = "Second of two self-named peers.")]
    async fn second_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("second")]))
    }
}

#[tool_handler]
impl ServerHandler for SecondNamedTool {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("second-peer", "1.0.0");
        info
    }
}

#[module(providers = [FirstNamedTool, SecondNamedTool])]
struct SelfNamedPeersModule;

#[tokio::test]
async fn self_named_peers_are_told_the_endpoint_reports_the_first() {
    let logs = LogCapture::install();
    let _app = TestApp::for_module::<SelfNamedPeersModule>()
        .await
        .expect("two self-named peers boot");

    let event = logs.expect_one("nest_rs::mcp", UNDECLARED_IDENTITY);
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reports_as").as_deref(),
        Some("first-peer"),
        "the message and the field say the same thing",
    );
    assert!(
        logs.find("nest_rs::mcp", SDK_DEFAULT_IDENTITY).is_empty(),
        "…and the fact that is *not* true here is not claimed: {:#?}",
        logs.events(),
    );
}

// --- a host that declares no router ------------------------------------------

/// Where the hand-written host and its router-backed peer share a mount.
const MANUAL: &str = "/manual";

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

    let manual = call_tool(app.http(), MANUAL, "manual_ping", None).await;
    assert!(
        manual.contains("manual here"),
        "a name no host claims through `get_tool` is offered to each in turn: {manual}",
    );

    let routed = call_tool(app.http(), MANUAL, "routed_ping", None).await;
    assert!(
        routed.contains("routed"),
        "…and the router-backed peer is still routed by name: {routed}",
    );
}

// --- distinct paths stay distinct -------------------------------------------

/// Where `OtherTool` lands: a second endpoint beside the default, the shape a
/// product reaches for when one feature deserves its own URL.
const OTHER: &str = "/mcp/other";

#[mcp(path = "/mcp/other")]
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
        hosts_on(app.container(), OTHER).len(),
        1,
        "the second path carries only its own",
    );

    let session = open_session(app.http(), OTHER, None).await;
    let body = call_method(app.http(), OTHER, &session, None, "tools/list", json!({})).await;
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

// --- where a host lands -------------------------------------------------------
//
// A `#[mcp]` path is the whole URL path, and omitting it takes the framework's
// default — a client is configured with a URL, not a prefix plus a segment.

/// The default is what an app that writes no path gets — including an app that
/// never imports `McpModule` at all, which is why it lives on the crate rather
/// than in configuration.
#[tokio::test]
async fn a_bare_host_serves_the_default_endpoint() {
    let app = TestApp::for_module::<AlphaOnlyApp>()
        .await
        .expect("an app with no McpModule import boots");

    assert_eq!(mcp_mount_paths(&app), [DEFAULT_PATH]);
    assert_eq!(
        DEFAULT_PATH, SHARED,
        "the suite's own constant and the crate's cannot drift",
    );

    let body = call_tool(app.http(), DEFAULT_PATH, "alpha_ping", None).await;
    assert!(
        body.contains("alpha here"),
        "the default endpoint answers on the wire, not just in the meta: {body}",
    );
}

/// A second endpoint is a second path, written whole — and it does not have to
/// sit under the first. Nothing nests below an MCP mount, so a host is free to
/// name any URL its clients will be configured with.
#[tokio::test]
async fn a_declared_path_is_served_verbatim() {
    let app = TestApp::for_module::<TwoPathApp>()
        .await
        .expect("two paths boot");

    let paths = mcp_mount_paths(&app);
    assert!(
        paths.contains(&SHARED.to_owned()) && paths.contains(&OTHER.to_owned()),
        "each host lands exactly where it said: {paths:?}",
    );
}

/// Every MCP mount an app assembled, as the transport sees it.
fn mcp_mount_paths(app: &TestApp) -> Vec<String> {
    DiscoveryService::new(app.container())
        .meta::<HttpEndpointMeta>()
        .into_iter()
        .filter(|discovered| discovered.meta.label() == "mcp")
        .map(|discovered| discovered.meta.path().to_owned())
        .collect()
}

// --- one endpoint, one handshake ----------------------------------------------
//
// A client negotiates the protocol version **once per endpoint**, so hosts that
// share a path share whatever the merge advertises: their intersection. That is
// the only answer a single handshake can give, and it silently narrows what a
// host declared — a host built against a newer version finds its endpoint
// speaking an older one because a peer it never heard of mounted beside it.

const VERSIONS: &str = "/mcp/versions";

/// Two versions apart. Alone it would advertise both.
#[mcp(path = "/mcp/versions")]
#[derive(Clone)]
struct LegacyVersionsTool;

#[tool_router]
impl LegacyVersionsTool {
    #[tool(description = "A tool from a host built against the older protocol.")]
    async fn legacy_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("legacy")]))
    }
}

#[tool_handler]
impl ServerHandler for LegacyVersionsTool {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(&[ProtocolVersion::V_2024_11_05, ProtocolVersion::V_2025_06_18])
    }
}

/// Its peer, overlapping in exactly one version — so the endpoint boots, and
/// the newest version this host declared is not the one it gets.
#[mcp(path = "/mcp/versions")]
#[derive(Clone)]
struct ModernVersionsTool;

#[tool_router]
impl ModernVersionsTool {
    #[tool(description = "A tool from a host built against the newer protocol.")]
    async fn modern_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("modern")]))
    }
}

#[tool_handler]
impl ServerHandler for ModernVersionsTool {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(&[ProtocolVersion::V_2025_06_18, ProtocolVersion::V_2025_11_25])
    }
}

#[module(providers = [LegacyVersionsTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct LegacyVersionsModule;

#[module(providers = [ModernVersionsTool])]
struct ModernVersionsModule;

#[module(imports = [LegacyVersionsModule, ModernVersionsModule])]
struct DisagreeingVersionsApp;

#[tokio::test]
async fn hosts_that_disagree_on_the_protocol_boot_on_their_intersection_and_say_so() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<DisagreeingVersionsApp>()
        .await
        .expect("an overlapping pair still negotiates one version");

    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp hosts on one path declare different protocol versions — the endpoint \
         advertises their intersection",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("path").as_deref(), Some(VERSIONS));
    // Both names, because neither host is at fault on its own: the narrowing is
    // a property of the pair, and an operator reading one name would go looking
    // for a bug in a host that declared exactly what it needed.
    let hosts = event.field("hosts").unwrap_or_default();
    assert!(
        hosts.contains("LegacyVersionsTool") && hosts.contains("ModernVersionsTool"),
        "the event names both hosts sharing the endpoint, got {hosts:?}",
    );

    // The endpoint answers, which is the half `initialize` can show: rmcp
    // exempts the handshake itself from the `supported_protocol_versions` check
    // (`uses_inline_negotiation` is false for an `InitializeRequest`), so what
    // comes back is the version the client asked for, not the intersection.
    // Asserting on it would be asserting rmcp's lifecycle.
    let handshake = initialize(app.http(), VERSIONS, None).await;
    assert!(
        result(&handshake)["result"]["capabilities"].is_object(),
        "the endpoint that warned still serves: {handshake}",
    );
}

/// A lone host advertises what it declared — the check has to be about the
/// pair, or every app that pins a protocol version warns at boot.
#[module(providers = [ModernVersionsTool])]
struct LoneVersionModule;

#[tokio::test]
async fn a_host_alone_on_its_path_narrows_nothing() {
    let logs = LogCapture::install();
    let _app = TestApp::for_module::<LoneVersionModule>()
        .await
        .expect("one host boots");

    assert!(
        logs.events()
            .iter()
            .all(|e| e.field("reason").as_deref() != Some("protocol_version_disagreement")),
        "{:#?}",
        logs.events(),
    );
}

/// Its twin, and the reason the warning is only a warning: hosts with *nothing*
/// in common describe an endpoint that can complete no handshake at all. Every
/// client would fail at `initialize`, which is a boot fact — so it is refused at
/// boot rather than discovered by a client.
#[mcp(path = "/mcp/incompatible")]
#[derive(Clone)]
struct AncientOnlyTool;

#[tool_router]
impl AncientOnlyTool {
    #[tool(description = "A tool from a host that speaks only the first protocol.")]
    async fn ancient_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("ancient")]))
    }
}

#[tool_handler]
impl ServerHandler for AncientOnlyTool {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(&[ProtocolVersion::V_2024_11_05])
    }
}

#[mcp(path = "/mcp/incompatible")]
#[derive(Clone)]
struct FutureOnlyTool;

#[tool_router]
impl FutureOnlyTool {
    #[tool(description = "A tool from a host that speaks only the latest protocol.")]
    async fn future_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("future")]))
    }
}

#[tool_handler]
impl ServerHandler for FutureOnlyTool {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }
}

#[module(providers = [AncientOnlyTool])]
struct AncientOnlyModule;

#[module(providers = [FutureOnlyTool])]
struct FutureOnlyModule;

#[module(imports = [AncientOnlyModule, FutureOnlyModule])]
struct IncompatibleVersionsApp;

#[tokio::test]
async fn hosts_with_no_protocol_in_common_fail_boot_naming_both() {
    let err = match TestApp::for_module::<IncompatibleVersionsApp>().await {
        Ok(_) => panic!("an endpoint no client can handshake with must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("AncientOnlyTool") && err.contains("FutureOnlyTool"),
        "the failure names both hosts: {err}",
    );
    assert!(
        err.contains("/mcp/incompatible"),
        "…and the endpoint they share: {err}",
    );
}

// --- a cursor the merge cannot honour -----------------------------------------
//
// Pagination is per host, and the merged page is not. A cursor names a position
// in *one* host's listing, so following it would return that host's next page
// with none of its peers' entries — and every entry already merged in, again.
// The merge therefore drops it, which turns a truncated listing into one that
// looks complete: a client sees no `nextCursor` and stops asking.

const PAGED: &str = "/mcp/paged";

/// A host with more resources than it returns at once — a perfectly ordinary
/// host, which is the point: it did nothing wrong and cannot know it is sharing.
#[mcp(path = "/mcp/paged")]
#[derive(Clone)]
struct PagedResourcesTool;

#[tool_router]
impl PagedResourcesTool {
    #[tool(description = "A tool beside a paginated resource listing.")]
    async fn paged_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("paged")]))
    }
}

#[tool_handler]
impl ServerHandler for PagedResourcesTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![Resource::new("paged://one", "Paged one")],
            next_cursor: Some("page-2".to_owned()),
            ..ListResourcesResult::default()
        })
    }
}

/// Its peer. Nothing about it is unusual either — sharing the path is the whole
/// of what the two of them did.
#[mcp(path = "/mcp/paged")]
#[derive(Clone)]
struct UnpagedResourcesTool;

#[tool_router]
impl UnpagedResourcesTool {
    #[tool(description = "A tool beside a complete resource listing.")]
    async fn unpaged_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("unpaged")]))
    }
}

#[tool_handler]
impl ServerHandler for UnpagedResourcesTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![Resource::new("plain://one", "Plain one")],
            ..ListResourcesResult::default()
        })
    }
}

#[module(providers = [PagedResourcesTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct PagedResourcesModule;

#[module(providers = [UnpagedResourcesTool])]
struct UnpagedResourcesModule;

#[module(imports = [PagedResourcesModule, UnpagedResourcesModule])]
struct PagedEndpointApp;

#[tokio::test]
async fn a_dropped_cursor_names_the_host_whose_listing_is_truncated() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<PagedEndpointApp>()
        .await
        .expect("a paginating host boots beside a peer");
    let session = open_session(app.http(), PAGED, None).await;

    let body = call_method(
        app.http(),
        PAGED,
        &session,
        None,
        "resources/list",
        json!({}),
    )
    .await;
    let listed = &result(&body)["result"];
    // Distinct schemes on purpose: with `paged://one` and `unpaged://one` the
    // first check was free — `"unpaged://one".contains("paged://one")` is true —
    // so dropping the paginating host's entries entirely still passed.
    assert!(
        body.contains("paged://one"),
        "the paginating host's entries reach the merged page: {body}",
    );
    assert!(
        body.contains("plain://one"),
        "…and so do its peer's: {body}",
    );
    assert!(
        listed
            .get("nextCursor")
            .is_none_or(serde_json::Value::is_null),
        "and the cursor is dropped rather than handed back pointing into one \
         host's listing: {body}",
    );

    let event = logs.expect_one(
        "nest_rs::mcp",
        "an MCP host on a shared path returned a pagination cursor — the merged page drops it",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("path").as_deref(), Some(PAGED));
    assert_eq!(
        event.field("host").as_deref(),
        Some("PagedResourcesTool"),
        "the truncated listing belongs to one host, and it is the one named: {:?}",
        event.fields,
    );
    assert_eq!(
        event.field("method").as_deref(),
        Some("resources/list"),
        "…on the listing method that truncated: {:?}",
        event.fields,
    );
}

/// The same host alone: its cursor is its own and a client can still follow it,
/// so nothing is dropped and nothing is said.
#[module(providers = [PagedResourcesTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct LonePagedModule;

#[tokio::test]
async fn a_lone_host_keeps_the_cursor_it_returned() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<LonePagedModule>()
        .await
        .expect("one paginating host boots");
    let session = open_session(app.http(), PAGED, None).await;

    let body = call_method(
        app.http(),
        PAGED,
        &session,
        None,
        "resources/list",
        json!({}),
    )
    .await;
    assert_eq!(
        result(&body)["result"]["nextCursor"].as_str(),
        Some("page-2"),
        "an unmerged listing paginates exactly as its host wrote it: {body}",
    );
    logs.expect_none(
        "nest_rs::mcp",
        "an MCP host on a shared path returned a pagination cursor — the merged page drops it",
    );
}
