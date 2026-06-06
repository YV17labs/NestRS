//! `scope_for` denies every row on request-scoped executors without an ability.

use nest_rs_authz::Action;
use nest_rs_seaorm::{Executor, scope_for, with_request_executor};
use sea_orm::{Database, EntityTrait, QueryFilter, QueryTrait};

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

#[tokio::test]
async fn request_scope_without_ability_denies_all_rows() {
    let url = std::env::var("NESTRS_DATABASE__URL")
        .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
    let conn = Database::connect(&url).await.expect("connect to Postgres");

    with_request_executor(Executor::Pool(conn), async {
        let sql = widget::Entity::find()
            .filter(scope_for::<widget::Entity>(Action::Read))
            .build(sea_orm::DatabaseBackend::Postgres)
            .to_string();
        assert!(
            sql.contains("1 = 0"),
            "request paths without an ability must fail closed: {sql}",
        );
    })
    .await;
}

#[tokio::test]
async fn job_scope_without_ability_remains_unscoped() {
    let url = std::env::var("NESTRS_DATABASE__URL")
        .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
    let conn = Database::connect(&url).await.expect("connect to Postgres");

    nest_rs_seaorm::with_job_executor(Executor::Pool(conn), async {
        let sql = widget::Entity::find()
            .filter(scope_for::<widget::Entity>(Action::Read))
            .build(sea_orm::DatabaseBackend::Postgres)
            .to_string();
        assert!(
            !sql.contains("1 = 0"),
            "worker paths stay unscoped when no ability is present: {sql}",
        );
    })
    .await;
}
