//! `soft_delete` and `timestamps` (`src/lifecycle.rs` in the macro crate): an
//! entity declared `soft_delete` registers its tombstone column, with or
//! without a service, so every `Repo` read hides its deleted rows. Postgres
//! proves the rows in the e2e suite; this proves the declaration.

use nest_rs_authz::Action;
use nest_rs_database::with_job_executor;
use nest_rs_seaorm::sea_orm::sea_query::SelectStatement;
use nest_rs_seaorm::sea_orm::{DatabaseBackend, QueryTrait};
use nest_rs_seaorm::{Repo, SeaOrmExecutor, SoftDeletable, live_condition};
use sea_orm::entity::prelude::*;

mod ledger {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "LedgerLine", soft_delete, timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "ledger_lines")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub created_at: DateTimeWithTimeZone,
        #[expose]
        pub updated_at: DateTimeWithTimeZone,
        #[sea_orm(column_name = "removed_at")]
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }
}

fn sql(select: impl QueryTrait<QueryStatement = SelectStatement>) -> String {
    select.build(DatabaseBackend::Postgres).to_string()
}

const LIVE: &str = r#""ledger_lines"."removed_at" IS NULL"#;

#[tokio::test]
async fn an_entity_declared_soft_delete_without_a_service_hides_its_tombstones() {
    let pool = SeaOrmExecutor::Pool(nest_rs_seaorm::sea_orm::DatabaseConnection::default());
    with_job_executor(pool.into(), async {
        let read = sql(Repo::<ledger::Entity>::scoped(Action::Read));
        assert!(read.contains(LIVE), "{read}");
        let escape = sql(Repo::<ledger::Entity>::with_tombstones(Action::Read));
        assert!(!escape.contains("IS NULL"), "{escape}");
    })
    .await;
    let unscoped = sql(Repo::<ledger::Entity>::unscoped());
    assert!(unscoped.contains(LIVE), "{unscoped}");
}

#[test]
fn the_tombstone_is_the_column_the_entity_maps_deleted_at_to() {
    assert!(matches!(
        ledger::Entity::deleted_at_column(),
        ledger::Column::DeletedAt
    ));
    let live = sql(ledger::Entity::find().filter(live_condition::<ledger::Entity>()));
    assert!(live.contains(LIVE), "{live}");
}
