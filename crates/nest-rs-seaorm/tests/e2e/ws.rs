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
use nest_rs_seaorm::{Executor, current_executor};
use nest_rs_ws::{Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::{ConnectionTrait, DatabaseConnection};

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
