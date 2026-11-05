//! A tool host over `SeaOrmDatabaseModule`: the MCP edge runs each operation as
//! a unit of work through the bound `dyn UnitOfWork`, with no data context
//! listed, under the ability the request's in-band guards kept, and commits
//! only an operation that answered `Ok` with no tool error in band.

use nest_rs_authz::{AbilityBuilder, Action, Read, current_ability, subject};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_database::{ExecutorScope, current_executor_scope};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_mcp::model::{CallToolResult, ContentBlock};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_seaorm::__private::{SeaOrmExecutor, current_executor};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool;
use sea_orm::ConnectionTrait;

/// What `inspect` reads: an operation naming no read reads under no ability.
#[subject]
struct Inspection;

/// Attaches an ability to every request, as the authorization guard does,
/// naming its caller as the authentication guard does.
#[injectable]
#[derive(Default)]
struct Granting;

impl Layer for Granting {}

#[nest_rs_core::async_trait]
impl Guard for Granting {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("granted");
        let mut ability = AbilityBuilder::new();
        ability.can(Action::Read, Inspection);
        let ability = ability.build().expect("one grant builds");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
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
    #[public(Read, Inspection, unmasked)]
    async fn inspect(&self) -> Result<CallToolResult, McpError> {
        let seen = format!(
            "lazy={} request={} ability={}",
            matches!(current_executor(), Some(SeaOrmExecutor::Lazy(_))),
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
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
    ],
    providers = [Granting, UnitTools],
)]
struct UnitModule;

#[module(imports = [UnitModule], providers = [GlobalGuards<(Granting,)>])]
struct BootApp;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<BootApp>()
        .build()
        .await
        .expect("the tool host boots over live Postgres")
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
    crate::harness::fresh_table(&conn, "mcp_unit_commit").await;
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
    crate::harness::fresh_table(&conn, "mcp_unit_tool_error").await;
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

const BLOCKED: &str = "/mcp/unit/blocked";

#[mcp(path = "/mcp/unit/blocked")]
#[derive(Clone, Default)]
struct BlockedTools;

#[tools]
impl BlockedTools {
    #[tool(description = "Writes to a table another transaction holds locked.")]
    #[public]
    async fn write_blocked(&self) -> Result<CallToolResult, McpError> {
        write("INSERT INTO mcp_unit_blocked (id) VALUES (1)").await;
        Ok(CallToolResult::success(vec![ContentBlock::text("written")]))
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        nest_rs_mcp::McpModule::for_root(nest_rs_mcp::McpConfig {
            operation_timeout: Some(std::time::Duration::from_secs(1)),
            ..nest_rs_mcp::McpConfig::default()
        }),
    ],
    providers = [Granting, BlockedTools],
)]
struct BlockedModule;

#[module(imports = [BlockedModule], providers = [GlobalGuards<(Granting,)>])]
struct AnOperationWhoseStatementIsBlockedIsAnsweredAtItsDeadlineApp;

/// An operation whose statement Postgres holds behind a lock is answered at
/// its deadline, not when the lock goes: the rollback waits for the statement
/// on the driver's task, and nothing it wrote lands.
#[tokio::test]
async fn an_operation_whose_statement_is_blocked_is_answered_at_its_deadline() {
    use sea_orm::TransactionTrait;

    let conn = crate::harness::connect().await;
    crate::harness::fresh_table(&conn, "mcp_unit_blocked").await;
    let locker = conn.begin().await.expect("the locking transaction opens");
    locker
        .execute_unprepared("LOCK TABLE mcp_unit_blocked IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("the table is locked");
    let app = TestApp::builder()
        .module::<AnOperationWhoseStatementIsBlockedIsAnsweredAtItsDeadlineApp>()
        .build()
        .await
        .expect("the tool host boots over live Postgres");

    // The lock goes only once the call is answered, so the deadline alone can
    // answer it; the bound turns a call waiting on the lock into a failure
    // rather than a hang.
    let call = call_tool(app.http(), BLOCKED, "write_blocked", None);
    let body = tokio::time::timeout(std::time::Duration::from_secs(60), call)
        .await
        .expect("answered while the lock is still held");
    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(
        answer["error"]["data"]["reason"], "timeout",
        "answered at its deadline: {answer}"
    );

    locker.rollback().await.expect("the lock goes");
    // The close drains the binding's settles: the count follows the cut unit's.
    app.close().await;
    assert_eq!(
        crate::harness::committed_rows(&conn, "mcp_unit_blocked").await,
        0,
        "the cut operation's write was rolled back once the statement ran",
    );
}

/// `#[authorize(A, bind = S)]` on a `#[resource]`, over the service's own SQL: the
/// row the ability admits reaches the body, and a row it refuses reads as one that
/// does not exist.
mod bound {
    use nest_rs_authz::{AbilityBuilder, Action, Read};
    use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
    use nest_rs_guards::{Denial, Guard, GuardFor};
    use nest_rs_http::{EndpointGuards, HttpView};
    use nest_rs_mcp::{Json, McpError, McpView, mcp, tools};
    use nest_rs_seaorm::{
        Authorized, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule,
    };
    use nest_rs_testing::TestApp;
    use nest_rs_testing::mcp::{call_method, open_session, result};
    use sea_orm::ConnectionTrait;
    use serde_json::{Value, json};
    use uuid::Uuid;

    mod note {
        use nest_rs_seaorm::expose;
        use sea_orm::entity::prelude::*;

        #[expose(name = "BoundNote")]
        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "mcp_bound_note")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            #[expose]
            pub id: Uuid,
            #[expose]
            pub owner: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    #[injectable]
    #[derive(Default)]
    struct NotesService;

    impl CrudService for NotesService {
        type Entity = note::Entity;
    }

    /// The ability a caller named `mine` holds: its own notes alone.
    #[injectable]
    #[derive(Default)]
    struct OwnNotes;

    impl Layer for OwnNotes {}

    #[async_trait]
    impl Guard for OwnNotes {
        fn __attaches_ability(&self) -> bool {
            true
        }

        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            nest_rs_core::__private::set_actor_id("mine");
            let mut ability = AbilityBuilder::new();
            ability
                .can(Action::Read, note::Entity)
                .when(|p| p.eq(note::Column::Owner, "mine"));
            cx.attach(nest_rs_authz::__private::AmbientAbility::new(
                ability.build().expect("a valid rule set"),
            ));
            Ok(())
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<HttpView> for OwnNotes {}

    const PATH: &str = "/mcp/bound-notes";

    #[nest_rs_core::input]
    struct NoteDto {
        id: Uuid,
        owner: String,
    }

    #[mcp(path = "/mcp/bound-notes")]
    #[derive(Clone, Default)]
    struct NotesTool;

    #[tools]
    impl NotesTool {
        /// A note, loaded and authorized.
        #[resource(template = "bound-note://{id}")]
        #[authorize(Read, bind = NotesService)]
        async fn note(
            &self,
            note: Authorized<Read, note::Entity>,
        ) -> Result<Json<NoteDto>, McpError> {
            let note = note.into_inner();
            Ok(Json(NoteDto {
                id: note.id,
                owner: note.owner,
            }))
        }
    }

    #[module(
        imports = [
            SeaOrmModule::for_root(SeaOrmConfig {
                url: crate::harness::url().into(),
                ..Default::default()
            }),
            SeaOrmDatabaseModule,
        ],
        providers = [OwnNotes, NotesService, NotesTool, EndpointGuards<McpView, (OwnNotes,)>],
    )]
    struct BoundModule;

    async fn read(app: &TestApp, session: &str, id: Uuid) -> Value {
        let body = call_method(
            app.http(),
            PATH,
            session,
            None,
            "resources/read",
            json!({ "uri": format!("bound-note://{id}") }),
        )
        .await;
        result(&body)
    }

    #[tokio::test]
    async fn the_row_the_ability_admits_is_read_and_a_refused_one_reads_as_missing() {
        let conn = crate::harness::connect().await;
        crate::harness::setup_shared_table(
            &conn,
            "mcp_bound_note",
            "CREATE TABLE IF NOT EXISTS mcp_bound_note (id uuid PRIMARY KEY, owner text NOT NULL);",
        )
        .await;
        let (mine, theirs) = (Uuid::now_v7(), Uuid::now_v7());
        conn.execute_unprepared(&format!(
            "INSERT INTO mcp_bound_note (id, owner) VALUES ('{mine}', 'mine'), ('{theirs}', 'theirs')"
        ))
        .await
        .expect("seed the notes");

        let app = TestApp::for_module::<BoundModule>()
            .await
            .expect("the tool host boots over live Postgres");
        let session = open_session(app.http(), PATH, None).await;

        let read_mine = read(&app, &session, mine).await;
        let text = read_mine["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("a text body: {read_mine}"));
        assert!(text.contains("mine"), "{read_mine}");

        let refused = read(&app, &session, theirs).await;
        let missing = read(&app, &session, Uuid::now_v7()).await;
        assert_eq!(
            refused["error"], missing["error"],
            "a reader cannot tell a row it was refused from a row that does not exist",
        );
        assert_eq!(refused["error"]["data"]["code"], "NOT_FOUND", "{refused}");
    }
}
