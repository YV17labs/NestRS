//! One vocabulary on `nest_rs::operation`, on every edge: `method` is the Rust
//! method wherever a line carries it, and a line names its unit by the words
//! its `mounted` line does, so a filter written on one reads the other.

use nest_rs_core::module;
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{Path, StatusCode, controller, routes};
use nest_rs_mcp::{McpAccess, McpError, McpModule, mcp, tools};
use nest_rs_testing::mcp::call_tool_statelessly;
use nest_rs_testing::{CapturedEvent, LogCapture, TestApp};

#[controller(path = "/shelves")]
#[derive(Default)]
struct ShelvesController;

#[routes]
impl ShelvesController {
    #[get("/{id}")]
    #[public]
    async fn read(&self, Path(id): Path<String>) -> String {
        id
    }
}

#[resolver]
struct ShelvesResolver;

#[operations]
impl ShelvesResolver {
    #[query]
    #[public]
    async fn list_shelves(&self) -> GqlResult<String> {
        Ok("shelves".into())
    }

    #[mutation]
    #[public]
    async fn clear_shelves(&self) -> GqlResult<String> {
        Ok("cleared".into())
    }
}

#[mcp]
#[derive(Clone, Default)]
struct ShelvesTool;

#[tools]
impl ShelvesTool {
    /// Answer.
    #[tool]
    #[public]
    async fn count_shelves(&self) -> Result<String, McpError> {
        Ok("3".to_owned())
    }
}

#[module(
    imports = [GraphqlModule, McpModule::for_root(McpAccess::Public)],
    providers = [ShelvesController, ShelvesResolver, ShelvesTool],
)]
struct ShelvesModule;

/// The `unit` line whose `field` reads `value`.
fn line(logs: &LogCapture, unit: &str, field: &str, value: &str) -> CapturedEvent {
    let lines = logs.find(nest_rs_core::operation_log::TARGET, unit);
    lines
        .iter()
        .find(|line| line.field(field).as_deref() == Some(value))
        .unwrap_or_else(|| panic!("a {unit} line with {field}={value}: {lines:#?}"))
        .clone()
}

/// The `mounted` line of `host`'s `method`.
fn mounted(logs: &LogCapture, host: &str, method: &str) -> CapturedEvent {
    let lines = logs.find(nest_rs_http::target::ROUTES, "mounted");
    lines
        .iter()
        .find(|line| {
            line.field("host").as_deref() == Some(host)
                && line.field("method").as_deref() == Some(method)
        })
        .unwrap_or_else(|| panic!("{host}::{method} mounted: {lines:#?}"))
        .clone()
}

fn assert_joins(line: &CapturedEvent, mounted: &CapturedEvent, fields: &[&str]) {
    for field in fields {
        assert!(line.field(field).is_some(), "{field}: {:?}", line.fields);
        assert_eq!(line.field(field), mounted.field(field), "{field}");
    }
}

#[tokio::test]
async fn a_request_line_says_its_verb_and_path_as_its_route_mounted_them() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ShelvesModule>().await.expect("boots");

    app.http()
        .get("/shelves/7")
        .send()
        .await
        .assert_status_is_ok();

    let line = line(&logs, nest_rs_http::unit::REQUEST.name(), "verb", "GET");
    assert_joins(
        &line,
        &mounted(&logs, "ShelvesController", "read"),
        &["verb", "path"],
    );
    assert_eq!(
        line.field("method"),
        None,
        "the handler's word: {:?}",
        line.fields
    );
    assert_eq!(
        line.field("route"),
        None,
        "`path` says it: {:?}",
        line.fields
    );
    assert!(!line.quotes("/shelves/7"), "{:?}", line.fields);
}

#[tokio::test]
async fn a_graphql_field_line_says_its_kind_and_field_as_its_host_mounted_them() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ShelvesModule>().await.expect("boots");

    for document in ["{ listShelves }", "mutation { clearShelves }"] {
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": document }))
            .send()
            .await
            .assert_status(StatusCode::OK);
    }

    for (kind, field, method) in [
        ("query", "listShelves", "list_shelves"),
        ("mutation", "clearShelves", "clear_shelves"),
    ] {
        let line = line(&logs, nest_rs_graphql::unit::FIELD.name(), "field", field);
        assert_eq!(
            line.field("kind").as_deref(),
            Some(kind),
            "{:?}",
            line.fields
        );
        assert_eq!(line.field("host").as_deref(), Some("ShelvesResolver"));
        assert_eq!(line.field("method").as_deref(), Some(method));
        assert_joins(
            &line,
            &mounted(&logs, "ShelvesResolver", method),
            &["kind", "field", "host", "method"],
        );
        assert_eq!(line.field("operation"), None, "{:?}", line.fields);
        assert_eq!(line.field("role"), None, "{:?}", line.fields);

        let document = line_of_operation(&logs, kind);
        assert_eq!(
            document.field("operation_type"),
            None,
            "{:?}",
            document.fields
        );
    }
}

/// The `graphql.operation` line of the operation type `kind`.
fn line_of_operation(logs: &LogCapture, kind: &str) -> CapturedEvent {
    line(logs, nest_rs_graphql::unit::OPERATION.name(), "kind", kind)
}

#[tokio::test]
async fn an_mcp_line_says_its_json_rpc_method_apart_from_the_handler_and_its_tool_as_mounted() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ShelvesModule>().await.expect("boots");

    let answered = call_tool_statelessly(
        app.http(),
        "/mcp",
        "count_shelves",
        None,
        &[],
        serde_json::json!({}),
    )
    .await;
    assert!(answered.contains('3'), "{answered}");

    let line = line(
        &logs,
        nest_rs_mcp::unit::OPERATION.name(),
        "mcp_method",
        "tools/call",
    );
    assert_joins(
        &line,
        &mounted(&logs, "ShelvesTool", "count_shelves"),
        &["tool"],
    );
    assert_eq!(
        line.field("method"),
        None,
        "the handler's word: {:?}",
        line.fields
    );
    assert_eq!(
        line.field("operation"),
        None,
        "`tool` says it: {:?}",
        line.fields
    );
}
