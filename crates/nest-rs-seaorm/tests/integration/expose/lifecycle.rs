//! The lifecycle `#[expose]` emits (`src/lifecycle.rs` in the macro crate): an
//! entity declared `soft_delete` registers its tombstone column, with or
//! without a service, so every `Repo` read hides its deleted rows; every
//! exposed entity registers its column defaults — the v7 key, a column's
//! `default` — for `Repo`'s inserts to fill where a row left them unset; and
//! `timestamps` writes the entity's `before_save`, which an entity without it
//! writes itself. Postgres proves the rows in the e2e suite; this proves the
//! declaration.

use std::any::TypeId;

use nest_rs_authz::Action;
use nest_rs_database::__private::with_job_executor;
use nest_rs_seaorm::__private::{ColumnDefaultRegistration, inventory};
use nest_rs_seaorm::sea_orm::sea_query::SelectStatement;
use nest_rs_seaorm::sea_orm::{DatabaseBackend, QueryTrait};
use nest_rs_seaorm::{Repo, SeaOrmExecutor, SoftDeletable, live_condition};
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, Set};

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

mod memo {
    use nest_rs_core::wire_enum;
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[wire_enum]
    #[derive(EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    pub(crate) enum Stage {
        #[sea_orm(string_value = "draft")]
        Draft,
        #[sea_orm(string_value = "sent")]
        Sent,
    }

    #[expose(name = "Memo", timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "memos")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub body: String,
        #[expose(default = Stage::Draft)]
        pub stage: Stage,
        #[expose]
        pub created_at: DateTimeWithTimeZone,
    }
}

mod hooked {
    use nest_rs_seaorm::expose;
    use sea_orm::ActiveValue;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Hooked")]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "hooked")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub name: String,
        #[expose(default = String::from("draft"))]
        pub stage: String,
    }

    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C>(mut self, _db: &C, _insert: bool) -> Result<Self, DbErr>
        where
            C: ConnectionTrait,
        {
            if let ActiveValue::Set(name) = &self.name {
                self.name = ActiveValue::Set(name.trim().to_owned());
            }
            Ok(self)
        }
    }
}

/// What an insert through `Repo` fills before its statement: the column
/// defaults the entity's `#[expose]` registered.
fn defaulted<A: 'static>(mut active: A) -> A {
    let registration = inventory::iter::<ColumnDefaultRegistration>
        .into_iter()
        .find(|entry| (entry.active_model)() == TypeId::of::<A>())
        .expect("`#[expose]` registers the entity's column defaults");
    (registration.fill)(&mut active);
    active
}

async fn saved<A: ActiveModelBehavior + Send>(active: A, insert: bool) -> A {
    active
        .before_save(&DatabaseConnection::default(), insert)
        .await
        .expect("before_save fills, it never fails")
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

#[test]
fn an_insert_leaving_the_key_unset_takes_a_v7_one() {
    let memo = defaulted(<memo::ActiveModel as Default>::default());
    let ActiveValue::Set(id) = memo.id else {
        panic!("the key is filled on insert: {:?}", memo.id);
    };
    assert_eq!(id.get_version_num(), 7);

    let ledger = defaulted(<ledger::ActiveModel as Default>::default());
    assert!(
        ledger.id.is_set(),
        "with or without a `default`: {ledger:?}"
    );
}

#[test]
fn a_column_default_fills_an_insert_that_left_the_column_unset() {
    let memo = defaulted(<memo::ActiveModel as Default>::default());
    assert_eq!(memo.stage, Set(memo::Stage::Draft));
}

#[test]
fn what_an_insert_carries_is_kept() {
    let id = Uuid::now_v7();
    let memo = defaulted(memo::ActiveModel {
        id: Set(id),
        stage: Set(memo::Stage::Sent),
        ..Default::default()
    });
    assert_eq!(memo.id, Set(id));
    assert_eq!(memo.stage, Set(memo::Stage::Sent));
}

#[tokio::test]
async fn an_entity_without_timestamps_keeps_its_own_hooks_beside_its_defaults() {
    let hooked = defaulted(hooked::ActiveModel {
        name: Set("  Ada  ".to_owned()),
        ..Default::default()
    });
    assert!(hooked.id.is_set(), "{hooked:?}");
    assert_eq!(hooked.stage, Set("draft".to_owned()));

    let saved = saved(hooked, true).await;
    assert_eq!(
        saved.name,
        Set("Ada".to_owned()),
        "the entity's own hook ran"
    );
}

#[tokio::test]
async fn timestamps_stamp_an_append_only_entitys_created_at_on_every_insert() {
    let before = nest_rs_seaorm::now();
    let memo = saved(<memo::ActiveModel as Default>::default(), true).await;
    let ActiveValue::Set(created_at) = memo.created_at else {
        panic!("`timestamps` stamps an append-only entity's created_at: {memo:?}");
    };
    assert!(created_at >= before);

    let then = before - chrono::TimeDelta::days(30);
    let carried = saved(
        memo::ActiveModel {
            created_at: Set(then),
            ..Default::default()
        },
        true,
    )
    .await;
    let ActiveValue::Set(stamped) = carried.created_at else {
        panic!("an insert is stamped whatever it carries: {carried:?}");
    };
    assert!(stamped >= before, "the insert's instant, not the row's");

    let updated = saved(<memo::ActiveModel as Default>::default(), false).await;
    assert_eq!(
        updated.created_at,
        ActiveValue::NotSet,
        "an update stamps none"
    );
}

#[tokio::test]
async fn updated_at_moves_on_every_save_and_created_at_only_on_insert() {
    let then = nest_rs_seaorm::now() - chrono::TimeDelta::days(30);
    let updated = saved(
        ledger::ActiveModel {
            created_at: Set(then),
            updated_at: Set(then),
            ..Default::default()
        },
        false,
    )
    .await;
    assert_eq!(updated.created_at, Set(then));
    let ActiveValue::Set(moved) = updated.updated_at else {
        panic!("an update stamps updated_at: {updated:?}");
    };
    assert!(moved > then);
}
