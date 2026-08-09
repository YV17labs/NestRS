//! `#[mcp]` on an `impl` block — the operations half of the decorator.
//!
//! Two Rust facts carry the design, and both are asserted here rather than
//! assumed. An inherent impl may be written in a **child module** of the one
//! defining the type, still reaching its private fields — which is what lets the
//! expansion keep `use rmcp;` out of the developer's file. And an item's own
//! visibility, not the module it sits in, decides who may name it — which is
//! what lets the boot checks still read a host's tool names, where rmcp's own
//! `tool_router()` (generated without `pub`) would have died at the module edge.

use nest_rs_core::module;
use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Role};
// Note what is *not* imported: `rmcp`, `tool`, `prompt`, `ServerHandler`.
// `#[tool]` and `#[prompt]` reach `#[mcp]` as inert tokens and are re-emitted
// inside the generated module, so they never need to be in scope here.
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::{AllowAllMcpGuard, McpError, McpOperationGuard, hosts_on, mcp};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_method, call_tool, initialize, open_session, result};

/// The endpoint this suite drives.
const PATH: &str = "/mcp/impl-witness";

/// A private dependency: the generated module must still reach it, which is the
/// first of the two facts.
#[derive(Clone)]
struct Directory(&'static [&'static str]);

#[mcp(path = "/mcp/impl-witness")]
#[derive(Clone)]
struct WitnessTool {
    people: Directory,
}

impl Default for WitnessTool {
    fn default() -> Self {
        Self {
            people: Directory(&["ada", "grace"]),
        }
    }
}

// One authored block. No `use rmcp`, no `#[tool_router]`, no `#[tool_handler]`,
// no `impl ServerHandler`, no `get_info`.
#[mcp]
impl WitnessTool {
    /// List everyone in the directory.
    #[tool]
    async fn list_people(&self) -> Result<String, McpError> {
        Ok(self.people.0.join("\n"))
    }

    /// Count the directory's entries.
    ///
    /// Carries the nested-meta form rmcp's own docs reach for: everything
    /// beside `description` has to survive the walk untouched, and a nested
    /// list is the shape that is easiest to drop on the floor.
    #[tool(annotations(title = "Count people", read_only_hint = true))]
    async fn count_people(&self) -> Result<String, McpError> {
        Ok(self.people.0.len().to_string())
    }

    /// Draft a greeting for the directory.
    #[prompt]
    async fn greet(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "hello",
        )]))
    }
}

#[module(providers = [WitnessTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct WitnessModule;

async fn boot() -> TestApp {
    TestApp::for_module::<WitnessModule>()
        .await
        .expect("a host whose operations live in one decorated impl boots")
}

/// Fact one: the generated impls sit in a child module and still reach the
/// host's private field.
#[tokio::test]
async fn the_tool_serves_from_a_generated_module() {
    let app = boot().await;

    let body = call_tool(app.http(), PATH, "list_people", None).await;
    assert!(
        body.contains("ada") && body.contains("grace"),
        "the tool body read a private field of a struct declared in the parent \
         module: {body}",
    );
}

/// Fact two, and the reason the expansion emits its own accessor: rmcp's
/// `tool_router()` is private to the module it is generated in, so the boot
/// checks would have seen an empty tool list and the duplicate-tool error would
/// have stopped firing.
#[tokio::test]
async fn the_boot_checks_still_read_the_tool_names() {
    let app = boot().await;

    let names: Vec<String> = hosts_on(app.container(), PATH)
        .first()
        .expect("the host contributes")
        .declared_tools()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect();

    assert_eq!(
        names,
        ["count_people", "list_people"],
        "static discovery survives the move into a private module",
    );
}

/// The prose was written twice — once for a reader, once for the model. The doc
/// comment is the copy a reader of the source sees, so it is the one that wins.
#[tokio::test]
async fn the_doc_comment_becomes_the_description_a_model_reads() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;

    let body = call_method(app.http(), PATH, &session, None, "tools/list", json!({})).await;
    assert!(
        body.contains("List everyone in the directory."),
        "the model reads the sentence the source already carried: {body}",
    );
    assert!(
        body.contains("Count the directory's entries."),
        "…including on an attribute that states other keys of its own: {body}",
    );
    assert!(
        body.contains("Count people"),
        "…and what that attribute stated survives the walk untouched: {body}",
    );
}

/// Capabilities are derived from the roles actually present, so a host cannot
/// route operations it forgot to advertise — the failure that shipped in the
/// CLI template and in every hand-written `impl ServerHandler for T {}`.
#[tokio::test]
async fn capabilities_are_derived_from_the_operations_present() {
    let app = boot().await;
    let body = initialize(app.http(), PATH, None).await;
    let advertised = &result(&body)["result"]["capabilities"];

    assert!(
        advertised["tools"].is_object(),
        "a `#[tool]` method advertises the tools capability: {advertised}",
    );
    assert!(
        advertised["prompts"].is_object(),
        "…and a `#[prompt]` method the prompts one: {advertised}",
    );
    assert!(
        advertised["resources"].is_null(),
        "…and nothing claims a surface no method serves: {advertised}",
    );
}

/// Both routers are fed from the one authored block, so a host that serves two
/// kinds of operation still writes one impl.
#[tokio::test]
async fn one_authored_impl_feeds_both_routers() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;

    let prompts = call_method(app.http(), PATH, &session, None, "prompts/list", json!({})).await;
    assert!(
        prompts.contains("greet"),
        "the prompt half of the same block is mounted too: {prompts}",
    );
}
