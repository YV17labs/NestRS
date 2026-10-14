//! A tool host over `SeaOrmDatabaseModule`: the MCP edge runs each operation as
//! a unit of work through the bound `dyn UnitOfWork`, with no data context
//! listed, under the ability the request's in-band guards kept, and commits
//! only an operation that answered `Ok` with no tool error in band.

use nest_rs_authz::{AbilityBuilder, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_mcp::{CallToolResult, ContentBlock, McpError, mcp, tools};
use nest_rs_seaorm::{
    Executor, ExecutorScope, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, current_executor,
    current_executor_scope,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool;
use sea_orm::{ConnectionTrait, DatabaseConnection};

/// Attaches an ability to every request, as the authorization guard does.
#[injectable]
#[derive(Default)]
struct Granting;

impl Layer for Granting {}

#[nest_rs_guards::async_trait]
impl Guard for Granting {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let ability = AbilityBuilder::new()
            .build()
            .expect("an ability with no rule builds");
        cx.attach(ability);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for Granting {}

async fn write(sql: &str) {
    current_executor()
        .expect("the operation runs with an ambient executor")
        .execute_unprepared(sql)
        .await
        .expect("the write lands in the operation's transaction");
}

const PATH: &str = "/mcp/unit";

#[mcp(path = "/mcp/unit")]
#[derive(Clone, Default)]
struct UnitTools;

#[tools]
impl UnitTools {
    #[tool(description = "Says what the operation runs on.")]
    #[public]
    async fn inspect(&self) -> Result<CallToolResult, McpError> {
        let seen = format!(
            "lazy={} request={} ability={}",
            matches!(current_executor(), Some(Executor::Lazy(_))),
            current_executor_scope() == Some(ExecutorScope::Request),
            current_ability().is_some(),
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(seen)]))
    }

    #[tool(description = "Writes, then succeeds.")]
    #[public]
    async fn succeed(&self) -> Result<CallToolResult, McpError> {
        write("INSERT INTO mcp_unit_commit (id) VALUES (1)").await;
        Ok(CallToolResult::success(vec![ContentBlock::text("written")]))
    }

    #[tool(description = "Writes, then reports the tool's own error.")]
    #[public]
    async fn report_error(&self) -> Result<CallToolResult, McpError> {
        write("INSERT INTO mcp_unit_tool_error (id) VALUES (1)").await;
        Ok(CallToolResult::error(vec![ContentBlock::text(
            "the tool failed after writing",
        )]))
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
    ],
    providers = [Granting, UnitTools],
)]
struct UnitModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<UnitModule>()
        .use_guards_global([guard::<Granting>()])
        .build()
        .await
        .expect("the tool host boots over live Postgres")
}

async fn table(conn: &DatabaseConnection, name: &str) {
    for sql in [
        format!("DROP TABLE IF EXISTS {name}"),
        format!("CREATE TABLE {name} (id INT PRIMARY KEY)"),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("set up the probe table `{name}`: {err}"));
    }
}

/// The text a tool answered with, and whether it reported an error.
async fn call(app: &TestApp, tool: &str) -> (String, bool) {
    let body = call_tool(app.http(), PATH, tool, None).await;
    let answer = nest_rs_testing::mcp::result(&body);
    let result = &answer["result"];
    (
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("the tool answered with text: {answer}"))
            .to_owned(),
        result["isError"] == true,
    )
}

#[tokio::test]
async fn a_host_listing_no_data_context_runs_each_operation_in_a_lazy_transaction() {
    let app = boot().await;
    assert_eq!(
        call(&app, "inspect").await,
        ("lazy=true request=true ability=true".to_owned(), false),
        "the edge opened the operation's unit, under the ability its guards kept",
    );
}

#[tokio::test]
async fn a_tool_that_succeeds_commits_what_it_wrote() {
    let conn = crate::harness::connect().await;
    table(&conn, "mcp_unit_commit").await;
    let app = boot().await;

    assert_eq!(call(&app, "succeed").await, ("written".to_owned(), false));
    assert_eq!(
        crate::harness::committed_rows(&conn, "mcp_unit_commit").await,
        1
    );
}

#[tokio::test]
async fn a_tool_that_reports_its_own_error_leaves_nothing_it_wrote() {
    let conn = crate::harness::connect().await;
    table(&conn, "mcp_unit_tool_error").await;
    let app = boot().await;

    assert_eq!(
        call(&app, "report_error").await,
        ("the tool failed after writing".to_owned(), true),
        "the tool's error reaches its client as the tool wrote it",
    );
    assert_eq!(
        crate::harness::committed_rows(&conn, "mcp_unit_tool_error").await,
        0,
        "a result with `isError: true` is the MCP specification's tool failure, never committed",
    );
}
