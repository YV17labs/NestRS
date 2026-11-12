//! `src/repo.rs`'s on-conflict insert against Postgres: a key no live row
//! holds inserts the row, one a live row holds returns that row, and neither
//! aborts the unit's transaction — the writes beside it commit, with no
//! savepoint around it. A soft-deletable entity's partial index over its live
//! rows is inferred, and a key only a tombstone holds meets its violation
//! inside a savepoint. An update setting nothing, for which sea-orm sends no
//! `UPDATE`, hands back only what its scope admits. A grant's `.fields` on an update
//! is decided in the statement's own `WHERE`: the columns it permits and the hook's
//! stamp land, and a row whose grants do not permit each column stays as it was. A
//! read gated on `Manage` holds each row to every concrete action, in its SQL.

use std::sync::Arc;

use nest_rs_authz::__private::install_ability;
use nest_rs_authz::{Ability, AbilityBuilder, Action};
use nest_rs_database::__private::with_request_executor;
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_seaorm::__private::SeaOrmExecutor;
use nest_rs_seaorm::{Insertion, Repo, RepoError, SeaOrmConfig, SeaOrmUnitOfWork};
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel,
    PaginatorTrait, QueryFilter, Set,
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

mod article {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "repo_probe_articles")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: Uuid,
        pub title: String,
        pub body: String,
        pub note: Option<String>,
        pub touched: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C: ConnectionTrait>(mut self, _: &C, _: bool) -> Result<Self, DbErr> {
            self.touched = sea_orm::Set(1);
            Ok(self)
        }
    }
}

/// The five tables: a plain unique key, one a hook normalises, a unique key over
/// live rows alone, a unique key counting tombstones, and a row a hook stamps.
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
        );
        CREATE TABLE IF NOT EXISTS repo_probe_articles (
            id UUID PRIMARY KEY,
            org_id UUID NOT NULL,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            note TEXT,
            touched INTEGER NOT NULL DEFAULT 0
        );",
    )
    .await;
    conn
}

/// `body` as one system unit of the binding, in a transaction of its own,
/// committed on `Ok`.
async fn in_a_unit<T: Send>(
    conn: &DatabaseConnection,
    body: impl Future<Output = Result<T, RepoError>> + Send,
) -> Result<T, RepoError> {
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
        |why| Err(DbErr::Custom(nest_rs_core::error_message(&why)).into()),
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
    assert!(
        matches!(&violation, RepoError::UniqueViolation(key) if key.table() == Some("repo_probe_legacy")),
        "{violation:?}"
    );
    let printed = nest_rs_testing::printed(&violation);
    assert!(printed.contains("23505"), "{printed}");
    assert!(
        !printed.contains(&email),
        "the server's detail quotes the row: {printed}"
    );
    assert_eq!(accounts_with(&conn, &after).await, 1);
}

#[tokio::test]
async fn an_update_setting_nothing_hands_back_only_a_row_its_scope_admits() {
    let conn = db().await;
    let own = account(&address("own"))
        .insert(&conn)
        .await
        .expect("the caller's row");
    let other = account(&address("other"))
        .insert(&conn)
        .await
        .expect("another caller's row");
    let email = own.email.clone();
    let mut grants = AbilityBuilder::new();
    grants
        .can(Action::Update, account::Entity)
        .when(move |p| p.eq(account::Column::Email, email));
    let ability = grants.build().expect("valid test ability");
    let by_key = |id| account::ActiveModel {
        id: Set(id),
        ..Default::default()
    };

    let (admitted, outside, absent) = under(&conn, ability, async {
        (
            Repo::<account::Entity>::update(by_key(own.id)).await,
            Repo::<account::Entity>::update(by_key(other.id)).await,
            Repo::<account::Entity>::update(by_key(Uuid::now_v7())).await,
        )
    })
    .await;

    assert_eq!(admitted.expect("its scope admits the row"), own);
    assert!(matches!(outside, Err(RepoError::NotFound)), "{outside:?}");
    assert!(matches!(absent, Err(RepoError::NotFound)), "{absent:?}");
}

#[tokio::test]
async fn an_update_setting_nothing_hides_a_tombstone_from_a_job_too() {
    let conn = db().await;
    let tombstone = member::ActiveModel {
        id: Set(Uuid::now_v7()),
        email: Set(address("tombstone")),
        deleted_at: Set(Some(nest_rs_seaorm::now())),
    }
    .insert(&conn)
    .await
    .expect("a tombstone");

    let met = in_a_unit(&conn, async {
        Ok(Repo::<member::Entity>::update(member::ActiveModel {
            id: Set(tombstone.id),
            ..Default::default()
        })
        .await)
    })
    .await
    .expect("the unit commits");

    assert!(matches!(met, Err(RepoError::NotFound)), "{met:?}");
}

/// `work`, in a request unit on the pool under `ability`.
async fn under<T>(conn: &DatabaseConnection, ability: Ability, work: impl Future<Output = T>) -> T {
    with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        install_ability(Arc::new(ability), work),
    )
    .await
}

fn granting(grants: impl FnOnce(&mut AbilityBuilder)) -> Ability {
    let mut b = AbilityBuilder::new();
    grants(&mut b);
    b.build().expect("valid test ability")
}

async fn an_article(conn: &DatabaseConnection, org_id: Uuid) -> article::Model {
    article::Entity::insert(article::ActiveModel {
        id: Set(Uuid::now_v7()),
        org_id: Set(org_id),
        title: Set("first".to_owned()),
        body: Set("as it stands".to_owned()),
        note: Set(None),
        touched: Set(0),
    })
    .exec_with_returning(conn)
    .await
    .expect("seed an article")
}

#[tokio::test]
async fn a_narrowed_update_writes_what_its_grant_permits_and_the_hooks_stamp() {
    let conn = db().await;
    let org = Uuid::now_v7();
    let row = an_article(&conn, org).await;
    let ability = granting(|b| {
        b.can(Action::Update, article::Entity)
            .when(|p| p.eq(article::Column::OrgId, org))
            .fields([article::Column::Title]);
    });
    let mut active = row.clone().into_active_model();
    active.title = Set("second".to_owned());

    let updated = under(&conn, ability, Repo::<article::Entity>::update(active))
        .await
        .expect("the grant permits the title");

    assert_eq!(updated.title, "second");
    assert_eq!(updated.touched, 1, "the hook's stamp needs no grant");
    assert_eq!(updated.body, row.body);
}

/// As a read's columns are the union of the grants its row meets, so a write's: a
/// row meeting one grant takes what that one permits, and only a row meeting both
/// takes both columns, in one statement, atomically.
#[tokio::test]
async fn a_row_is_written_only_where_its_grants_permit_each_column() {
    let conn = db().await;
    let (titles, bodies) = (Uuid::now_v7(), Uuid::now_v7());
    let in_titles = an_article(&conn, titles).await;
    let in_bodies = an_article(&conn, bodies).await;
    let two_grants = || {
        granting(|b| {
            b.can(Action::Update, article::Entity)
                .when(|p| p.eq(article::Column::OrgId, titles))
                .fields([article::Column::Title]);
            b.can(Action::Update, article::Entity)
                .when(|p| p.eq(article::Column::OrgId, bodies))
                .fields([article::Column::Body]);
        })
    };
    let title_of = |row: &article::Model| {
        let mut active = row.clone().into_active_model();
        active.title = Set("second".to_owned());
        active
    };

    let written = under(
        &conn,
        two_grants(),
        Repo::<article::Entity>::update(title_of(&in_titles)),
    )
    .await
    .expect("the row's grant permits it");
    assert_eq!(written.title, "second");

    let outside = under(
        &conn,
        two_grants(),
        Repo::<article::Entity>::update(title_of(&in_bodies)),
    )
    .await;
    assert!(matches!(outside, Err(RepoError::NotFound)), "{outside:?}");
    let stands = article::Entity::find_by_id(in_bodies.id)
        .one(&conn)
        .await
        .expect("read an article");
    assert_eq!(stands.as_ref(), Some(&in_bodies));

    let mut both = title_of(&in_titles);
    both.body = Set("rewritten".to_owned());
    let no_row_meets_both = under(&conn, two_grants(), Repo::<article::Entity>::update(both)).await;
    assert!(
        matches!(no_row_meets_both, Err(RepoError::NotFound)),
        "{no_row_meets_both:?}",
    );
}

#[tokio::test]
async fn an_update_under_no_field_list_writes_every_column_it_sets() {
    let conn = db().await;
    let org = Uuid::now_v7();
    let row = an_article(&conn, org).await;
    let ability = granting(|b| {
        b.can(Action::Update, article::Entity)
            .when(|p| p.eq(article::Column::OrgId, org));
    });
    let mut active = row.clone().into_active_model();
    active.body = Set("rewritten".to_owned());
    active.note = Set(Some("added".to_owned()));

    let updated = under(&conn, ability, Repo::<article::Entity>::update(active))
        .await
        .expect("no list, no limit");

    assert_eq!(updated.body, "rewritten");
    assert_eq!(updated.note.as_deref(), Some("added"));
}

/// Read every row of the org, change only one's: the rows a `Manage` read admits are
/// the ones every concrete action admits, decided by the database.
fn managing_one(org: Uuid, mine: Uuid) -> Ability {
    granting(|b| {
        for action in [Action::Read, Action::Create] {
            b.can(action, article::Entity)
                .when(move |p| p.eq(article::Column::OrgId, org));
        }
        for action in [Action::Update, Action::Delete] {
            b.can(action, article::Entity)
                .when(move |p| p.eq(article::Column::Id, mine));
        }
    })
}

#[tokio::test]
async fn a_read_gated_on_manage_admits_the_rows_every_action_admits() {
    let conn = db().await;
    let org = Uuid::now_v7();
    let mine = an_article(&conn, org).await;
    let theirs = an_article(&conn, org).await;

    let (managed, read) = under(&conn, managing_one(org, mine.id), async {
        let in_org = || article::Column::OrgId.eq(org);
        let managed = Repo::<article::Entity>::scoped(Action::Manage)
            .filter(in_org())
            .all()
            .await
            .expect("the managed rows are read");
        let read = Repo::<article::Entity>::scoped(Action::Read)
            .filter(in_org())
            .count()
            .await
            .expect("the readable rows are counted");
        (managed, read)
    })
    .await;

    assert_eq!(read, 2, "`Read` admits both: {theirs:?}");
    assert_eq!(managed, [mine], "`Update` and `Delete` admit one row");
}

#[tokio::test]
async fn an_instance_check_on_manage_answers_per_row_in_sql() {
    let conn = db().await;
    let org = Uuid::now_v7();
    let mine = an_article(&conn, org).await;
    let theirs = an_article(&conn, org).await;

    let (on_mine, on_theirs) = under(&conn, managing_one(org, mine.id), async {
        (
            Repo::<article::Entity>::can(Action::Manage, &mine).await,
            Repo::<article::Entity>::can(Action::Manage, &theirs).await,
        )
    })
    .await;

    assert!(on_mine.expect("the check runs"));
    assert!(!on_theirs.expect("the check runs"));
}
