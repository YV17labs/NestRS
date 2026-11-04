//! `src/repo.rs`'s on-conflict insert against Postgres: a key no live row
//! holds inserts the row, one a live row holds returns that row, and neither
//! aborts the unit's transaction — the writes beside it commit, with no
//! savepoint around it. A soft-deletable entity's partial index over its live
//! rows is inferred, and a key only a tombstone holds meets its violation
//! inside a savepoint.

use std::sync::Arc;

use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_seaorm::{DbOutcome, Insertion, Repo, SeaOrmConfig, SeaOrmUnitOfWork};
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};

mod account {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "repo_probe_accounts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod normalised {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "repo_probe_normalised")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C: ConnectionTrait>(mut self, _: &C, _: bool) -> Result<Self, DbErr> {
            if let sea_orm::ActiveValue::Set(email) = &self.email {
                self.email = sea_orm::ActiveValue::Set(email.to_lowercase());
            }
            Ok(self)
        }
    }
}

mod member {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Member", soft_delete)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "repo_probe_members")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub email: String,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod legacy {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Legacy", soft_delete)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "repo_probe_legacy")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub email: String,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// The four tables: a plain unique key, one a hook normalises, a unique key over
/// live rows alone, and a unique key counting tombstones.
async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "repo_probe_accounts",
        "CREATE TABLE IF NOT EXISTS repo_probe_accounts (
            id UUID PRIMARY KEY,
            email TEXT NOT NULL CONSTRAINT repo_probe_accounts_email_key UNIQUE
        );
        CREATE TABLE IF NOT EXISTS repo_probe_normalised (
            id UUID PRIMARY KEY,
            email TEXT NOT NULL CONSTRAINT repo_probe_normalised_email_key UNIQUE
        );
        CREATE TABLE IF NOT EXISTS repo_probe_members (
            id UUID PRIMARY KEY,
            email TEXT NOT NULL,
            deleted_at TIMESTAMPTZ
        );
        CREATE UNIQUE INDEX IF NOT EXISTS repo_probe_members_email_live
            ON repo_probe_members (email) WHERE deleted_at IS NULL;
        CREATE TABLE IF NOT EXISTS repo_probe_legacy (
            id UUID PRIMARY KEY,
            email TEXT NOT NULL CONSTRAINT repo_probe_legacy_email_key UNIQUE,
            deleted_at TIMESTAMPTZ
        );",
    )
    .await;
    conn
}

/// `body` as one system unit of the binding, in a transaction of its own,
/// committed on `Ok`.
async fn in_a_unit<T: Send>(
    conn: &DatabaseConnection,
    body: impl Future<Output = Result<T, DbErr>> + Send,
) -> Result<T, DbErr> {
    let binding: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(
        Arc::new(conn.clone()),
        &SeaOrmConfig::default(),
    ));
    run_unit(
        Some(&binding),
        UnitSpec::new(
            nest_rs_core::unit::HOOK,
            ExecutorScope::Job,
            TransactionPolicy::PerUnit,
        ),
        body,
        Result::is_ok,
        |why| Err(DbErr::Custom(nest_rs_core::error_message(&why))),
    )
    .await
}

fn account(email: &str) -> account::ActiveModel {
    account::ActiveModel {
        id: Set(Uuid::now_v7()),
        email: Set(email.to_owned()),
    }
}

async fn accounts_with(conn: &DatabaseConnection, email: &str) -> u64 {
    account::Entity::find()
        .filter(account::Column::Email.eq(email))
        .count(conn)
        .await
        .expect("the probe rows are counted")
}

fn address(what: &str) -> String {
    format!("{what}-{}@probe.test", Uuid::now_v7())
}

#[tokio::test]
async fn a_key_no_live_row_holds_inserts_the_row() {
    let conn = db().await;
    let email = address("fresh");

    let inserted = in_a_unit(&conn, async {
        Repo::<account::Entity>::insert_unscoped_or_existing(
            account(&email),
            [account::Column::Email],
        )
        .await
    })
    .await
    .expect("the unit commits");

    let Insertion::New(row) = inserted else {
        panic!("no row held the key: {inserted:?}");
    };
    assert_eq!(row.email, email);
    assert_eq!(accounts_with(&conn, &email).await, 1);
}

/// The conflict is absorbed by the statement: the writes on either side of it
/// commit with the unit, which a raised violation would have aborted.
#[tokio::test]
async fn a_key_a_live_row_holds_returns_that_row_and_aborts_nothing() {
    let conn = db().await;
    let taken = address("taken");
    let held = account(&taken)
        .insert(&conn)
        .await
        .expect("a row holds the key");
    let (before, after) = (address("before"), address("after"));

    let met = in_a_unit(&conn, async {
        Repo::<account::Entity>::insert_unscoped(account(&before)).await?;
        let met = Repo::<account::Entity>::insert_unscoped_or_existing(
            account(&taken),
            [account::Column::Email],
        )
        .await?;
        Repo::<account::Entity>::insert_unscoped(account(&after)).await?;
        Ok(met)
    })
    .await
    .expect("the unit commits: no statement in it failed");

    assert_eq!(met, Insertion::Existing(held));
    for email in [&taken, &before, &after] {
        assert_eq!(accounts_with(&conn, email).await, 1, "{email}");
    }
}

/// The insert sends what the entity's `before_save` returned, so the row holding
/// the key is read by that spelling, never the one the caller wrote.
#[tokio::test]
async fn a_key_a_hook_normalises_returns_the_row_holding_what_it_wrote() {
    let conn = db().await;
    let email = address("mixed");
    let written = |email: String| normalised::ActiveModel {
        id: Set(Uuid::now_v7()),
        email: Set(email),
    };

    let (first, second) = in_a_unit(&conn, async {
        let key = [normalised::Column::Email];
        let first =
            Repo::<normalised::Entity>::insert_unscoped_or_existing(written(email.clone()), key)
                .await?;
        let second = Repo::<normalised::Entity>::insert_unscoped_or_existing(
            written(email.to_uppercase()),
            key,
        )
        .await?;
        Ok((first, second))
    })
    .await
    .expect("the unit commits: no statement in it failed");

    let Insertion::New(row) = first else {
        panic!("no row held the key: {first:?}");
    };
    assert_eq!(second, Insertion::Existing(row));
}

/// A partial index over live rows is inferred, so a tombstone holding the key
/// is no conflict, and the live row the insert made is the one met next.
#[tokio::test]
async fn a_soft_deletable_entitys_live_key_is_inferred_and_a_tombstone_never_met() {
    let conn = db().await;
    let email = address("member");
    member::ActiveModel {
        id: Set(Uuid::now_v7()),
        email: Set(email.clone()),
        deleted_at: Set(Some(nest_rs_seaorm::now())),
    }
    .insert(&conn)
    .await
    .expect("a tombstone holds the address");
    let fresh = || member::ActiveModel {
        email: Set(email.clone()),
        ..Default::default()
    };

    let (first, second) = in_a_unit(&conn, async {
        let first =
            Repo::<member::Entity>::insert_unscoped_or_existing(fresh(), [member::Column::Email])
                .await?;
        let second =
            Repo::<member::Entity>::insert_unscoped_or_existing(fresh(), [member::Column::Email])
                .await?;
        Ok((first, second))
    })
    .await
    .expect("the unit commits");

    let Insertion::New(live) = first else {
        panic!("a tombstone holds no live key: {first:?}");
    };
    assert_eq!(live.deleted_at, None);
    assert_eq!(second, Insertion::Existing(live));
}

/// A unique key counting tombstones meets one the read does not return: the
/// insert is sent again inside a savepoint, and its violation answers while
/// the unit's other writes still commit.
#[tokio::test]
async fn a_key_only_a_tombstone_holds_meets_its_violation_and_aborts_nothing() {
    let conn = db().await;
    let email = address("legacy");
    legacy::ActiveModel {
        id: Set(Uuid::now_v7()),
        email: Set(email.clone()),
        deleted_at: Set(Some(nest_rs_seaorm::now())),
    }
    .insert(&conn)
    .await
    .expect("a tombstone holds the address");
    let after = address("after");

    let met = in_a_unit(&conn, async {
        let met = Repo::<legacy::Entity>::insert_unscoped_or_existing(
            legacy::ActiveModel {
                email: Set(email.clone()),
                ..Default::default()
            },
            [legacy::Column::Email],
        )
        .await;
        Repo::<account::Entity>::insert_unscoped(account(&after)).await?;
        Ok(met)
    })
    .await
    .expect("the unit commits: the violation stayed in its savepoint");

    let Err(violation) = met else {
        panic!("a tombstone is never returned: {met:?}");
    };
    assert_eq!(
        DbOutcome::of(&violation),
        DbOutcome::Conflict,
        "{violation}"
    );
    assert_eq!(accounts_with(&conn, &after).await, 1);
}
