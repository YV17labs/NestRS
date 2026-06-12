//! [`Repo`] — the query entry point that makes security and transactions
//! transparent. Every call runs against the ambient
//! [`Executor`](crate::Executor) (the request's transaction when open) and is
//! filtered by the caller's [`Ability`](nest_rs_authz::Ability): reads via
//! `condition_for(Read)`, by-id writes via `condition_for(Update/Delete)` ANDed
//! with the primary key — so a caller cannot mutate a row outside its scope
//! even by id. Worker/system paths run unscoped (no ability ⇒ TRUE); a
//! request-scoped executor without an ability denies every row (fail-closed).
//! A call outside the executor scope errors rather than silently reaching a
//! connection it does not have.

use std::marker::PhantomData;

use nest_rs_authz::{Action, current_ability};
use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{
    ActiveModelTrait, DbErr, Delete, DeleteResult, EntityTrait, IntoActiveModel, PrimaryKeyTrait,
    QueryFilter, Select, Update, Value,
};

use crate::executor::{Executor, ExecutorScope, current_executor, current_executor_scope};

/// Row-level filter for `action` on `E` from the ambient ability. Deny-all on a
/// request-scoped executor without an ability; unscoped on worker/system paths.
pub fn scope_for<E: EntityTrait>(action: Action) -> Condition {
    match current_ability() {
        Some(ability) => ability.condition_for::<E>(action),
        None if current_executor_scope() == Some(ExecutorScope::Request) => {
            tracing::warn!(
                target: "nest_rs::orm",
                entity = std::any::type_name::<E>(),
                ?action,
                "no ambient Ability on a request-scoped executor — denying all rows",
            );
            Condition::all().add(Expr::cust("1 = 0"))
        }
        None => Condition::all(),
    }
}

/// Repository over entity `E`, bound to the ambient request executor and ability.
pub struct Repo<E: EntityTrait>(PhantomData<fn() -> E>);

impl<E: EntityTrait> Repo<E> {
    /// The ambient executor (transaction when open, else the pool), for a write
    /// or a custom query: `active.insert(&Repo::<E>::conn()?)`.
    pub fn conn() -> Result<Executor, DbErr> {
        current_executor().ok_or_else(|| {
            DbErr::Custom(
                "no ambient database executor — a Repo query must run inside the request \
                 scope installed by nestrs-seaorm's DbContext interceptor"
                    .to_owned(),
            )
        })
    }

    /// Every row of `E` the caller may [`Read`](Action::Read).
    pub async fn all() -> Result<Vec<E::Model>, DbErr> {
        let conn = Self::conn()?;
        E::find()
            .filter(scope_for::<E>(Action::Read))
            .all(&conn)
            .await
    }

    /// A row by primary key, returned only if the caller may
    /// [`Read`](Action::Read) it. A row outside the caller's scope reads as
    /// `None` — never leaking its existence.
    pub async fn find_by_id(
        id: <E::PrimaryKey as PrimaryKeyTrait>::ValueType,
    ) -> Result<Option<E::Model>, DbErr> {
        let conn = Self::conn()?;
        E::find_by_id(id)
            .filter(scope_for::<E>(Action::Read))
            .one(&conn)
            .await
    }

    /// A [`Select`] pre-filtered to what the caller may `action`, for a custom
    /// query. Chain further constraints and execute against [`Repo::conn`].
    pub fn scoped(action: Action) -> Select<E> {
        E::find().filter(scope_for::<E>(action))
    }

    /// A [`Select`] that **bypasses the ambient ability filter** — for the two
    /// sanctioned ability-less query paths:
    ///
    /// 1. **Pre-authentication** credential lookup, which runs before any
    ///    principal (hence any ability) exists — routing it through
    ///    [`scoped`](Self::scoped) on a request-scoped executor would deny every
    ///    row (`scope_for` fail-closed), making login impossible.
    /// 2. **Access binding** (`CrudService::access`), which is deliberately
    ///    unscoped so a denied-but-existing row reports `Denied` rather than
    ///    `Missing`; the ability check is then applied explicitly per row.
    ///
    /// Still runs against the ambient [`Repo::conn`] executor, so it participates
    /// in the request transaction — only the row-level scope is dropped. Reach
    /// for this **only** in those two cases; every other read must use
    /// [`scoped`](Self::scoped).
    pub fn unscoped() -> Select<E> {
        E::find()
    }

    /// The by-primary-key analog of [`unscoped`](Self::unscoped) — a
    /// `find_by_id` [`Select`] with **no** ability filter, for `CrudService::access`
    /// (see the second sanctioned case in [`unscoped`](Self::unscoped)). Chain
    /// the soft-delete / live filter and execute against [`Repo::conn`].
    pub fn unscoped_by_id(
        id: <E::PrimaryKey as PrimaryKeyTrait>::ValueType,
    ) -> Select<E> {
        E::find_by_id(id)
    }

    /// Update a row, gated by `condition_for(Update)` ANDed with the primary
    /// key: a row outside the caller's scope is never touched and surfaces as
    /// [`DbErr::RecordNotUpdated`], so a caller cannot mutate by id past its
    /// scope.
    pub async fn update<A>(active: A) -> Result<E::Model, DbErr>
    where
        A: ActiveModelTrait<Entity = E> + Send,
        E::Model: IntoActiveModel<A>,
    {
        let conn = Self::conn()?;
        Update::one(active)
            .validate()?
            .filter(scope_for::<E>(Action::Update))
            .exec(&conn)
            .await
    }

    /// Delete a row, gated by `condition_for(Delete)` ANDed with the primary
    /// key. A row outside the caller's scope is not deleted; the returned
    /// [`DeleteResult::rows_affected`] is `0` when the scope (or the row's
    /// absence) excluded it — the caller decides whether that is a denial.
    pub async fn delete<A, M>(model: M) -> Result<DeleteResult, DbErr>
    where
        A: ActiveModelTrait<Entity = E> + Send,
        M: IntoActiveModel<A> + Send,
    {
        let conn = Self::conn()?;
        Delete::one(model)
            .validate()?
            .filter(scope_for::<E>(Action::Delete))
            .exec(&conn)
            .await
    }

    /// Soft-delete a loaded row: stamp `col = now()` in the request transaction,
    /// gated by `condition_for(Delete)` ANDed with the primary key. Idempotent
    /// when the row is already tombstoned. Hard purge stays on [`Self::delete`].
    pub async fn soft_delete<A, M>(model: M, col: E::Column) -> Result<(), DbErr>
    where
        A: ActiveModelTrait<Entity = E> + Send,
        M: IntoActiveModel<A> + Send,
        E::Model: IntoActiveModel<A>,
    {
        let conn = Self::conn()?;
        let mut active = model.into_active_model();
        let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().fixed_offset();
        active.set(col, Value::from(now));
        Update::one(active)
            .validate()?
            .filter(scope_for::<E>(Action::Delete))
            .exec(&conn)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_authz::{AbilityBuilder, with_ability};
    use sea_orm::sea_query::{Condition, Expr};
    use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};

    use super::*;
    use crate::executor::{Executor, with_job_executor, with_request_executor};
    use crate::soft_delete::live_condition;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod tombstone {
        use sea_orm::entity::prelude::*;

        use crate::soft_delete::SoftDeletable;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "tombstones")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub deleted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}

        impl SoftDeletable for Entity {
            fn deleted_at_column() -> Column {
                Column::DeletedAt
            }
        }
    }

    fn sql(cond: Condition) -> String {
        widget::Entity::find()
            .filter(cond)
            .build(DatabaseBackend::Postgres)
            .to_string()
    }

    // `Condition` carries an opaque builder; build SQL from a stub query to
    // peek at the rendered shape without a real DB. Reusing the same trick
    // the scope/e2e tests use, but here against a freshly-installed
    // task-local — no Postgres needed.
    #[tokio::test]
    async fn no_ambient_state_renders_unscoped() {
        // Outside any executor scope: ambient ability is absent and scope is
        // `None` (not `Request`), so the engine returns `TRUE`.
        let s = sql(scope_for::<widget::Entity>(Action::Read));
        assert!(!s.contains("1 = 0"), "no scope ⇒ unscoped: {s}");
    }

    // A request-scoped executor with NO ambient ability must fail closed.
    // A bug that defaults to `TRUE` here leaks every row to every caller.
    #[tokio::test]
    async fn request_scope_without_ability_denies_all_rows() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_request_executor(pool, async {
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(s.contains("1 = 0"), "request paths fail closed: {s}");
        })
        .await;
    }

    // System work (workers, schedule, shutdown hooks) runs unscoped — that's
    // the documented invariant, and the regression check is paranoid.
    #[tokio::test]
    async fn job_scope_without_ability_remains_unscoped() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_job_executor(pool, async {
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(!s.contains("1 = 0"), "worker paths stay unscoped: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn an_ambient_ability_wins_over_the_scope_default() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 7));
        let ability = Arc::new(b.build());

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let s = sql(scope_for::<widget::Entity>(Action::Read));
                assert!(s.contains("org_id"), "ability condition is applied: {s}");
                assert!(!s.contains("1 = 0"));
            })
            .await;
        })
        .await;
    }

    // The `Repo::conn` error message names the interceptor, so a developer
    // reading the failure trail knows *why* the call was outside scope —
    // pinning the message is a cheap teaching invariant.
    #[test]
    fn repo_conn_outside_scope_names_the_interceptor() {
        let msg = match Repo::<widget::Entity>::conn() {
            Ok(_) => panic!("expected error outside the executor scope"),
            Err(err) => err.to_string(),
        };
        assert!(msg.contains("ambient"), "missing 'ambient': {msg}");
        assert!(msg.contains("DbContext"), "missing 'DbContext': {msg}");
    }

    // Sanity: `Condition::all().add(Expr::cust("1 = 0"))` (the deny-all clause)
    // serializes with the literal substring `1 = 0` — this is what the e2e
    // test asserts on, pinned here so a sea_query rename surfaces immediately.
    #[test]
    fn deny_all_condition_serializes_as_one_equals_zero() {
        let s = sql(Condition::all().add(Expr::cust("1 = 0")));
        assert!(s.contains("1 = 0"), "got: {s}");
    }

    fn select_sql(select: Select<widget::Entity>) -> String {
        select.build(DatabaseBackend::Postgres).to_string()
    }

    // `Repo::scoped` is the entry point for a custom query: a `Select<E>`
    // already filtered by the ambient ability. Without a scope installed,
    // the engine returns `TRUE`, so the rendered SQL has no `1 = 0` guard.
    #[tokio::test]
    async fn scoped_renders_unscoped_outside_a_request() {
        let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
        assert!(!s.contains("1 = 0"), "no scope ⇒ unscoped: {s}");
    }

    // `Repo::scoped` inside a request scope without an ability must inherit
    // the deny-all guard from `scope_for` — a developer that builds a custom
    // query off `scoped` cannot accidentally bypass row-level security.
    #[tokio::test]
    async fn scoped_in_request_without_ability_renders_deny_all() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_request_executor(pool, async {
            let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
            assert!(s.contains("1 = 0"), "scoped() fails closed too: {s}");
        })
        .await;
    }

    // An ability that grants the action unconditionally renders as a `TRUE`
    // predicate — the canonical "admin" shape.
    #[tokio::test]
    async fn scoped_with_unconditional_grant_renders_unrestricted() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        let ability = Arc::new(b.build());

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
                // No deny-all clause: an unconditional grant lets every row through.
                assert!(!s.contains("1 = 0"), "admin should not be denied: {s}");
            })
            .await;
        })
        .await;
    }

    // The four actions are independent rule keys: a per-action predicate must
    // appear on its own action and not leak to the others. A bug that keyed
    // all actions to one bucket would surface as identical SQL across calls.
    #[tokio::test]
    async fn scope_for_per_action_uses_distinct_predicates() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 1));
        b.can(Action::Create, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 2));
        b.can(Action::Update, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 3));
        b.can(Action::Delete, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 4));
        let ability = Arc::new(b.build());

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let read = sql(scope_for::<widget::Entity>(Action::Read));
                let create = sql(scope_for::<widget::Entity>(Action::Create));
                let update = sql(scope_for::<widget::Entity>(Action::Update));
                let delete = sql(scope_for::<widget::Entity>(Action::Delete));

                assert!(read.contains('1'), "Read keyed to org_id = 1: {read}");
                assert!(create.contains('2'), "Create keyed to org_id = 2: {create}");
                assert!(update.contains('3'), "Update keyed to org_id = 3: {update}");
                assert!(delete.contains('4'), "Delete keyed to org_id = 4: {delete}");

                // And the four must not collapse to the same SQL — a regression
                // that lost the action discriminator would fail here.
                assert_ne!(read, create);
                assert_ne!(read, update);
                assert_ne!(read, delete);
                assert_ne!(create, update);
            })
            .await;
        })
        .await;
    }

    // An action the ability does not mention falls through to the deny-all
    // clause on a request-scoped executor: silently allowing it would be a
    // privilege-escalation regression.
    #[tokio::test]
    async fn scope_for_denies_an_unmentioned_action_inside_request() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        let ability = Arc::new(b.build());

        with_request_executor(pool, async move {
            with_ability(ability, async {
                // The ability grants only `Read`; `Delete` is not mentioned,
                // so the ambient condition for `Delete` must deny every row.
                let s = sql(scope_for::<widget::Entity>(Action::Delete));
                assert!(s.contains("1 = 0"), "missing action denies: {s}");
            })
            .await;
        })
        .await;
    }

    // `Repo::conn` returns the installed executor when one is present. The
    // request and job variants take separate code paths in `current_executor_scope`;
    // both must observe the same executor under `Repo::conn`.
    #[tokio::test]
    async fn repo_conn_returns_the_installed_request_executor() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_request_executor(pool, async {
            let conn = Repo::<widget::Entity>::conn().expect("an executor is installed");
            assert!(matches!(conn, Executor::Pool(_)));
        })
        .await;
    }

    #[tokio::test]
    async fn repo_conn_returns_the_installed_job_executor() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_job_executor(pool, async {
            let conn = Repo::<widget::Entity>::conn().expect("an executor is installed");
            assert!(matches!(conn, Executor::Pool(_)));
        })
        .await;
    }

    #[test]
    fn live_condition_renders_deleted_at_is_null() {
        let s = tombstone::Entity::find()
            .filter(live_condition::<tombstone::Entity>())
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            s.to_ascii_lowercase().contains("deleted_at"),
            "live filter must target deleted_at: {s}",
        );
        assert!(
            s.contains("NULL"),
            "live filter must exclude tombstoned rows: {s}",
        );
    }
}
