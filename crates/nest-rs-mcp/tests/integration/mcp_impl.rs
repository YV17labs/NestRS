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
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpOperationGuard, Parameters, Valid, hosts_on, input, mcp, tools,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{
    call_method, call_tool, call_tool_with, initialize, open_session, result,
};

/// The endpoint this suite drives.
const PATH: &str = "/mcp/impl-witness";

/// A private dependency: the generated module must still reach it, which is the
/// first of the two facts.
#[derive(Clone)]
struct Directory(&'static [&'static str]);

/// A tool's typed arguments, validated by the pipe the operation declares.
#[input]
struct GreetArgs {
    #[validate(length(min = 1, message = "name must not be empty"))]
    name: String,
}

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
#[tools]
impl WitnessTool {
    /// List everyone in the directory.
    #[tool]
    #[public]
    async fn list_people(&self) -> Result<String, McpError> {
        Ok(self.people.0.join("\n"))
    }

    /// Count the directory's entries.
    ///
    /// Carries the nested-meta form rmcp's own docs reach for: everything
    /// beside `description` has to survive the walk untouched, and a nested
    /// list is the shape that is easiest to drop on the floor.
    #[tool(annotations(title = "Count people", read_only_hint = true))]
    #[public]
    async fn count_people(&self) -> Result<String, McpError> {
        Ok(self.people.0.len().to_string())
    }

    /// Greet one person by name.
    #[tool]
    #[public]
    async fn greet_person(
        &self,
        Parameters(args): Parameters<Valid<GreetArgs>>,
    ) -> Result<String, McpError> {
        Ok(format!("hello {}", args.into_inner().name))
    }

    #[tool(
        description = "Report how the directory is stored. Stated on the attribute, with no \
                       doc comment above it."
    )]
    #[public]
    async fn describe_storage(&self) -> Result<String, McpError> {
        Ok("in memory".to_owned())
    }

    /// Draft a greeting for the directory.
    #[prompt]
    #[public]
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
        [
            "count_people",
            "describe_storage",
            "greet_person",
            "list_people"
        ],
        "static discovery survives the move into a private module — and the wire \
         name is the authored one, not the wrapper ident the expansion routes \
         through",
    );
}

/// A pipe on an operation's arguments behaves as it does on every other
/// transport: the carrier never reaches the wire, and a rejection is the one
/// error a model can act on.
#[tokio::test]
async fn a_valid_argument_is_validated_before_the_body_runs() {
    let app = boot().await;

    let ok = call_tool_with(
        app.http(),
        PATH,
        "greet_person",
        None,
        json!({ "name": "ada" }),
    )
    .await;
    assert!(
        ok.contains("hello ada"),
        "a valid argument reaches the body: {ok}"
    );

    let rejected = call_tool_with(
        app.http(),
        PATH,
        "greet_person",
        None,
        json!({ "name": "" }),
    )
    .await;
    assert!(
        rejected.contains("name must not be empty"),
        "…and an invalid one is refused with the field error, so the model can \
         correct the argument it got wrong: {rejected}",
    );
    assert!(
        !rejected.contains("hello"),
        "…without the body ever running: {rejected}",
    );
}

/// The pipe carrier is the *body's* type, never the schema's — a client asked to
/// send a `Valid<GreetArgs>` would have nothing it could construct.
#[tokio::test]
async fn the_wire_schema_is_the_value_type_not_the_carrier() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;

    let body = call_method(app.http(), PATH, &session, None, "tools/list", json!({})).await;
    let listed = result(&body);
    let greet = listed["result"]["tools"]
        .as_array()
        .expect("tools/list carries an array")
        .iter()
        .find(|tool| tool["name"] == "greet_person")
        .expect("the piped tool is listed")
        .clone();

    let schema = &greet["inputSchema"];
    assert!(
        schema["properties"]["name"].is_object(),
        "the schema is `GreetArgs`'s own: {schema}",
    );
    assert!(
        !schema.to_string().contains("Valid"),
        "…and the carrier appears nowhere in it — a client asked to send a \
         `Valid<GreetArgs>` would have nothing it could construct: {schema}",
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

/// The other direction, and the one `demo/` relies on: an operation may state
/// its description on the attribute and carry no doc comment at all. Nothing
/// fails to compile when that regresses — the model would just read a wrong or
/// empty sentence — so it is asserted rather than assumed.
#[tokio::test]
async fn a_description_stated_on_the_attribute_needs_no_doc_comment() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;

    let body = call_method(app.http(), PATH, &session, None, "tools/list", json!({})).await;
    assert!(
        body.contains("Report how the directory is stored."),
        "the attribute's own `description` reaches the model: {body}",
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
