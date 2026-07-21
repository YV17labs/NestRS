//! `scope_for` denies every row on request-scoped executors without an
//! ability — asserted against **executed** queries on live Postgres, not
//! rendered SQL strings.

use nest_rs_authz::Action;
use nest_rs_seaorm::{Executor, scope_for, with_request_executor};
use sea_orm::{DatabaseConnection, EntityTrait, QueryFilter};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "scope_probe_widgets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Connect and make sure the probe table exists with its two rows. The DDL +
/// seed are serialized in Postgres, not per process — nextest gives each test
/// its own, so concurrent `CREATE TABLE IF NOT EXISTS` would race the catalog.
async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "scope_probe_widgets",
        "CREATE TABLE IF NOT EXISTS scope_probe_widgets (
            id INT PRIMARY KEY,
            name TEXT NOT NULL
        );
         INSERT INTO scope_probe_widgets (id, name) VALUES (1, 'a'), (2, 'b')
         ON CONFLICT (id) DO NOTHING;",
    )
    .await;
    conn
}

#[tokio::test]
async fn request_scope_without_ability_denies_all_rows() {
    let conn = db().await;

    with_request_executor(Executor::Pool(conn.clone()), async {
        let rows = widget::Entity::find()
            .filter(scope_for::<widget::Entity>(Action::Read))
            .all(&conn)
            .await
            .expect("the scoped query executes");
        assert!(
            rows.is_empty(),
            "request paths without an ability must fail closed, got {} row(s)",
            rows.len(),
        );
    })
    .await;
}

#[tokio::test]
async fn job_scope_without_ability_remains_unscoped() {
    let conn = db().await;

    nest_rs_seaorm::with_job_executor(Executor::Pool(conn.clone()), async {
        let rows = widget::Entity::find()
            .filter(scope_for::<widget::Entity>(Action::Read))
            .all(&conn)
            .await
            .expect("the scoped query executes");
        assert!(
            rows.len() >= 2,
            "worker paths stay unscoped when no ability is present, got {} row(s)",
            rows.len(),
        );
    })
    .await;
}
