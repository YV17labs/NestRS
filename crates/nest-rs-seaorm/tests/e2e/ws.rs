//! `WsDataContext` installs a **lazy per-message transaction** and the
//! caller's ambient `Ability` around each gateway message dispatch: a
//! non-querying message opens nothing, a writing handler commits on a
//! success reply and rolls back on an error reply. Gated on the `ws`
//! feature.

#![cfg(feature = "ws")]

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, current_ability};
use nest_rs_core::Container;
use nest_rs_seaorm::ws::WsDataContext;
use nest_rs_seaorm::{
    Executor, ExecutorScope, current_executor, current_executor_scope, scope_for,
};
use nest_rs_ws::{Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryTrait};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn ability() -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 1));
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn capture_reads_the_upgrade_ability_and_installs_a_lazy_executor() {
    let container = Container::builder()
        .provide_arc(crate::harness::connect_arc().await)
        .build();
    let ctx = WsDataContext::from_container(&container);

    let mut req = Request::default();
    req.extensions_mut().insert(ability());

    let captured = ctx.capture(&req);
    ctx.around(
        &captured,
        Box::pin(async {
            let executor = current_executor().expect("executor installed per message");
            assert!(
                matches!(executor, Executor::Lazy(_)),
                "WS messages run on a lazy per-message transaction",
            );
            assert!(
                current_ability().is_some(),
                "the upgrade ability is re-installed per message",
            );
            WsReply::None
        }),
    )
    .await;
}

#[tokio::test]
async fn around_without_an_upgrade_ability_still_installs_the_executor() {
    let container = Container::builder()
        .provide_arc(crate::harness::connect_arc().await)
        .build();
    let ctx = WsDataContext::from_container(&container);

    let captured = ctx.capture(&Request::default());
    ctx.around(
        &captured,
        Box::pin(async {
            let executor = current_executor().expect("executor installed per message");
            executor
                .execute_unprepared("SELECT 1")
                .await
                .expect("a live query runs through the lazily opened transaction");
            assert!(
                current_ability().is_none(),
                "guest connections have no ambient ability",
            );
            WsReply::None
        }),
    )
    .await;
}

/// A gateway whose module bound no authz module reads through `Repo` with no
/// ambient ability. It must fail closed **and say so**: a WS handler has no
/// status code and no response envelope, so the empty array is the entire
/// signal and is indistinguishable from an empty table. The claim under test is
/// that this transport is not quieter than HTTP — the executor it installs is
/// request-scoped, which is what puts the deny branch (and its `warn`) in play,
/// rather than the unscoped worker branch that would have returned every row.
#[tokio::test]
async fn an_ability_less_read_over_websockets_denies_loudly() {
    let container = Container::builder()
        .provide_arc(crate::harness::connect_arc().await)
        .build();
    let ctx = WsDataContext::from_container(&container);
    let logs = nest_rs_testing::LogCapture::install();

    let captured = ctx.capture(&Request::default());
    ctx.around(
        &captured,
        Box::pin(async {
            assert_eq!(
                current_executor_scope(),
                Some(ExecutorScope::Request),
                "a message is request-scoped work, never system work",
            );
            let sql = probe::Entity::find()
                .filter(scope_for::<probe::Entity>(Action::Read))
                .build(sea_orm::DatabaseBackend::Postgres)
                .to_string();
            assert!(sql.contains("1 = 0"), "fails closed: {sql}");
            WsReply::None
        }),
    )
    .await;

    let event = logs.expect_one(
        "nest_rs::orm",
        "no ambient Ability outside a worker job — denying all rows",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("action").as_deref(), Some("Read"));
}

/// Stand-in entity for the scope probe above — the filter is rendered, never
/// executed, so no table has to exist.
mod probe {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "ws_scope_probe")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

async fn probe_table(conn: &DatabaseConnection, name: &str) {
    conn.execute_unprepared(&format!("DROP TABLE IF EXISTS {name}"))
        .await
        .expect("drop leftover probe table");
    conn.execute_unprepared(&format!("CREATE TABLE {name} (id INT PRIMARY KEY)"))
        .await
        .expect("create probe table");
}

async fn count_rows(conn: &DatabaseConnection, name: &str) -> i32 {
    use sea_orm::{DatabaseBackend, Statement};
    conn.query_one_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!("SELECT COUNT(*)::int AS n FROM {name}"),
    ))
    .await
    .expect("count query")
    .expect("count row")
    .try_get("", "n")
    .expect("n column")
}

// The D3 contract: a writing handler whose reply is an error must not
// half-persist — the per-message transaction rolls its writes back.
#[tokio::test]
async fn an_error_reply_rolls_back_the_messages_writes() {
    let conn = crate::harness::connect_arc().await;
    probe_table(&conn, "ws_rollback_probe").await;

    let container = Container::builder().provide_arc(conn.clone()).build();
    let ctx = WsDataContext::from_container(&container);
    let captured = ctx.capture(&Request::default());

    let reply = ctx
        .around(
            &captured,
            Box::pin(async {
                let executor = current_executor().expect("executor installed");
                executor
                    .execute_unprepared("INSERT INTO ws_rollback_probe (id) VALUES (1)")
                    .await
                    .expect("the write lands in the message transaction");
                WsReply::error("handler failed mid-way")
            }),
        )
        .await;

    assert!(matches!(reply, WsReply::Error(_)));
    assert_eq!(
        count_rows(&conn, "ws_rollback_probe").await,
        0,
        "an error reply must roll back the message's writes",
    );
    conn.execute_unprepared("DROP TABLE IF EXISTS ws_rollback_probe")
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn a_success_reply_commits_the_messages_writes() {
    let conn = crate::harness::connect_arc().await;
    probe_table(&conn, "ws_commit_probe").await;

    let container = Container::builder().provide_arc(conn.clone()).build();
    let ctx = WsDataContext::from_container(&container);
    let captured = ctx.capture(&Request::default());

    let reply = ctx
        .around(
            &captured,
            Box::pin(async {
                let executor = current_executor().expect("executor installed");
                executor
                    .execute_unprepared("INSERT INTO ws_commit_probe (id) VALUES (1)")
                    .await
                    .expect("the write lands in the message transaction");
                WsReply::None
            }),
        )
        .await;

    assert!(matches!(reply, WsReply::None));
    assert_eq!(
        count_rows(&conn, "ws_commit_probe").await,
        1,
        "a success reply must commit the message's writes",
    );
    conn.execute_unprepared("DROP TABLE IF EXISTS ws_commit_probe")
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn a_mismatched_captured_context_runs_bare() {
    let container = Container::builder()
        .provide_arc(crate::harness::connect_arc().await)
        .build();
    let ctx = WsDataContext::from_container(&container);
    let bad: Captured = Arc::new(());

    ctx.around(
        &bad,
        Box::pin(async {
            assert!(
                current_executor().is_none(),
                "unexpected capture must not install ambient state",
            );
            WsReply::None
        }),
    )
    .await;
}

/// A message whose `COMMIT` the database refuses.
///
/// `dispatch::with_data_context` is one seam serving WS and MCP, so this is the
/// commit-failure branch for both — and the reason it must not pass the
/// handler's reply through is the same as everywhere else: the client is told
/// the message was handled, and nothing it wrote is there. A socket makes that
/// worse than a request does, because the next message arrives on the same
/// connection and looks like it is continuing from a state that never existed.
#[tokio::test]
async fn a_message_whose_commit_the_database_refuses_is_not_replied_to_as_a_success() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect_arc().await;
    crate::harness::deferred_probe_tables(&conn, "ws_deferred").await;

    let container = Container::builder().provide_arc(conn.clone()).build();
    let ctx = WsDataContext::from_container(&container);
    let captured = ctx.capture(&Request::default());

    let reply = ctx
        .around(
            &captured,
            Box::pin(async {
                let executor = current_executor().expect("executor installed");
                executor
                    .execute_unprepared(
                        "INSERT INTO ws_deferred_children (id, parent_id) VALUES (1, 4242)",
                    )
                    .await
                    .expect("a deferred constraint lets the statement through");
                // The handler's own answer is a success.
                WsReply::None
            }),
        )
        .await;

    assert!(
        matches!(reply, WsReply::Error(_)),
        "a reply whose transaction did not commit is turned into an error frame",
    );
    assert_eq!(
        count_rows(&conn, "ws_deferred_children").await,
        0,
        "and nothing was written",
    );

    let event = logs.expect_one("nest_rs::orm", "dispatch transaction commit failed");
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("transport").as_deref(),
        Some("ws"),
        "which edge, since one seam settles both in-band transports: {:?}",
        event.fields,
    );
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("ws_deferred")),
        "…and the database's own reason, got {:?}",
        event.fields,
    );
}
