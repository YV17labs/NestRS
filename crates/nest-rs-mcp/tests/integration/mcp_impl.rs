//! `#[tools]`: its impls sit in a generated child module, which still reaches the
//! host's private fields and exposes the router `pub(crate)` to the boot checks.

use nest_rs_core::module;
use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Role};
// No `rmcp`, `tool`, `prompt` or `ServerHandler` import: `#[tools]` re-emits
// them inside its generated module.
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpOperationGuard, Parameters, Valid, hosts_on, input, mcp, tools,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{
    call_method, call_tool, call_tool_with, initialize, open_session, result,
};

const PATH: &str = "/mcp/impl-witness";

/// A private field the generated module must still reach.
#[derive(Clone)]
struct Directory(&'static [&'static str]);

/// A description stated through a constant: a value only the compiler
/// evaluates, so the expansion checks it in a `const` rather than at expansion.
const RETENTION: &str = "Report how long the directory keeps an entry.";

/// A `Result` under another name: `#[tools]` reads the answer by type.
type Answered<T> = Result<T, McpError>;

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

    /// Look up one person.
    #[doc = concat!("Only ", "an admin may widen the search.")]
    #[tool]
    #[public]
    async fn look_up_person(&self) -> Result<String, McpError> {
        Ok("found".to_owned())
    }

    #[doc = concat!("Audit ", "the directory.")]
    #[tool]
    #[public]
    async fn audit_directory(&self) -> Result<String, McpError> {
        Ok("audited".to_owned())
    }

    #[tool(
        description = "Report how the directory is stored. Stated on the attribute, with no \
                       doc comment above it."
    )]
    #[public]
    async fn describe_storage(&self) -> Result<String, McpError> {
        Ok("in memory".to_owned())
    }

    #[tool(description = RETENTION)]
    #[public]
    async fn describe_retention(&self) -> Result<String, McpError> {
        Ok("forever".to_owned())
    }

    /// Greet one person by name, through a `Result` renamed by an alias.
    #[tool]
    #[public]
    async fn greet_by_alias(
        &self,
        Parameters(args): Parameters<Valid<GreetArgs>>,
    ) -> Answered<String> {
        Ok(format!("hi {}", args.into_inner().name))
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

/// rmcp generates `tool_router()` private to its module unless asked otherwise;
/// a private one would leave the duplicate-tool boot check an empty list.
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
            "audit_directory",
            "count_people",
            "describe_retention",
            "describe_storage",
            "greet_by_alias",
            "greet_person",
            "list_people",
            "look_up_person",
        ],
        "static discovery survives the move into a private module — and the wire \
         name is the authored one, not the wrapper ident the expansion routes \
         through",
    );
}

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

#[tokio::test]
async fn a_result_renamed_by_an_alias_answers_and_refuses_like_a_spelled_one() {
    let app = boot().await;

    let ok = call_tool_with(
        app.http(),
        PATH,
        "greet_by_alias",
        None,
        json!({ "name": "ada" }),
    )
    .await;
    assert!(ok.contains("hi ada"), "the operation answers: {ok}");

    let rejected = call_tool_with(
        app.http(),
        PATH,
        "greet_by_alias",
        None,
        json!({ "name": "" }),
    )
    .await;
    assert!(
        rejected.contains("name must not be empty") && !rejected.contains("hi "),
        "…and its pipe's refusal is the operation's answer: {rejected}",
    );
}

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

#[tokio::test]
async fn a_macro_valued_doc_line_is_part_of_the_description() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;

    let body = call_method(app.http(), PATH, &session, None, "tools/list", json!({})).await;
    assert!(
        body.contains("Look up one person. Only an admin may widen the search."),
        "the macro-valued line joins the literal one: {body}",
    );
    assert!(
        body.contains("Audit the directory."),
        "a doc written only as a macro is a doc: {body}",
    );
}

/// `demo/` relies on this, and a regression still compiles.
#[tokio::test]
async fn a_description_stated_on_the_attribute_needs_no_doc_comment() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;

    let body = call_method(app.http(), PATH, &session, None, "tools/list", json!({})).await;
    assert!(
        body.contains("Report how the directory is stored."),
        "the attribute's own `description` reaches the model: {body}",
    );
    assert!(
        body.contains(RETENTION),
        "…and so does one stated through a constant: {body}",
    );
}

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

/// A host whose only tool is compiled out: `#[tools]` reads the method before
/// `#[cfg]` is evaluated.
#[mcp(path = "/mcp/impl-witness/prompts-only")]
#[derive(Clone, Default)]
struct PromptsOnlyTool;

#[tools]
impl PromptsOnlyTool {
    #[cfg(any())]
    #[tool(description = "Not in this build.")]
    #[public]
    async fn compiled_out(&self) -> Result<String, McpError> {
        Ok(String::new())
    }

    #[prompt(description = "Draft a greeting.")]
    #[public]
    async fn greet(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "hello",
        )]))
    }
}

#[test]
fn a_capability_whose_every_method_is_compiled_out_is_not_advertised() {
    let capabilities = nest_rs_mcp::ServerHandler::get_info(&PromptsOnlyTool).capabilities;
    assert!(
        capabilities.tools.is_none(),
        "no `#[tool]` survives `#[cfg]`, so nothing claims the tools surface",
    );
    assert!(
        capabilities.prompts.is_some(),
        "…while the `#[prompt]` that does survive still advertises its own",
    );
}
