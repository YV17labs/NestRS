//! `WsDataContext` installs a pool executor and the caller's ambient `Ability`
//! around each gateway message dispatch. Gated on the `ws` feature.

#![cfg(feature = "ws")]

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, current_ability};
use nest_rs_core::Container;
use nest_rs_seaorm::ws::WsDataContext;
use nest_rs_seaorm::{Executor, current_executor};
use nest_rs_ws::{Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};

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

async fn db() -> Arc<DatabaseConnection> {
    let url = std::env::var("NESTRS_DATABASE__URL")
        .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
    Arc::new(Database::connect(&url).await.expect("connect to Postgres"))
}

fn ability() -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 1));
    Arc::new(b.build())
}

#[tokio::test]
async fn capture_reads_the_upgrade_ability_and_pool_executor() {
    let container = Container::builder().provide_arc(db().await).build();
    let ctx = WsDataContext::from_container(&container);

    let mut req = Request::default();
    req.extensions_mut().insert(ability());

    let captured = ctx.capture(&req);
    ctx.around(
        &captured,
        Box::pin(async {
            let executor = current_executor().expect("executor installed per message");
            assert!(
                matches!(executor, Executor::Pool(_)),
                "WS messages run on the shared pool",
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
async fn around_without_an_upgrade_ability_still_installs_the_pool() {
    let container = Container::builder().provide_arc(db().await).build();
    let ctx = WsDataContext::from_container(&container);

    let captured = ctx.capture(&Request::default());
    ctx.around(
        &captured,
        Box::pin(async {
            let executor = current_executor().expect("pool executor installed");
            executor
                .execute_unprepared("SELECT 1")
                .await
                .expect("a live query runs through the pool");
            assert!(
                current_ability().is_none(),
                "guest connections have no ambient ability",
            );
            WsReply::None
        }),
    )
    .await;
}

#[tokio::test]
async fn a_mismatched_captured_context_runs_bare() {
    let container = Container::builder().provide_arc(db().await).build();
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
