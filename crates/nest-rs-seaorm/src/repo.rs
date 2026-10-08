use std::marker::PhantomData;

use nest_rs_authz::{Action, current_ability};
use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ConnectionTrait, DbErr, Delete, DeleteResult,
    EntityTrait, IntoActiveModel, PrimaryKeyTrait, QueryFilter, Select, Update, Value,
};

use crate::executor::{Executor, ExecutorScope, current_executor, current_executor_scope};

/// Row-level filter for `action` on `E` from the ambient ability. A worker/system
/// (`Job`) executor is the sole unscoped path; every other path without an ability
/// — request-scoped or untagged — denies all rows (fail-closed).
pub fn scope_for<E: EntityTrait>(action: Action) -> Condition {
    match current_ability() {
        Some(ability) => ability.condition_for::<E>(action),
        None if current_executor_scope() == Some(ExecutorScope::Job) => Condition::all(),
        None => {
            let scope = current_executor_scope();
            // The route answers `200 []`, which reads as a business bug: the
            // event is the only place the cause shows.
            tracing::warn!(
                target: crate::TARGET,
                entity = std::any::type_name::<E>(),
                ?action,
                scope = ?scope,
                hint = "bind #[use_guards(AuthnGuard, AuthzGuard)], or — on a #[public] \
                          route — take the `Authorize<A, E>` parameter that arms the shaper",
                "no ambient Ability outside a worker job — denying all rows",
            );
            Condition::all().add(Expr::cust("1 = 0"))
        }
    }
}

/// Repository over entity `E`, bound to the ambient request executor and ability.
pub struct Repo<E: EntityTrait>(PhantomData<fn() -> E>);

impl<E: EntityTrait> Repo<E> {
    /// The ambient executor (transaction when open, else the pool), to run a
    /// custom read you filter yourself, e.g.
    /// `Repo::<E>::scoped(Action::Read).one(&Repo::<E>::conn()?)`.
    ///
    /// A **write** must go through the service write path
    /// ([`create_from_active`](crate::Creatable::create_from_active) or the
    /// `Creatable`/`Updatable`/`Deletable` traits), never
    /// `active.insert(&Repo::<E>::conn()?)`: a raw insert skips the ability
    /// pre-filter, committing an out-of-scope row.
    pub fn conn() -> Result<Executor, DbErr> {
        current_executor().ok_or_else(|| {
            // Logged: on the wire it is the constant `"database error"`.
            const HINT: &str = "a Repo query runs against the executor its transport installs: \
                 HTTP through SeaOrmDatabaseModule's DbContext interceptor, a WS message through \
                 `WsDataContext as dyn SocketContext`, an MCP tool through \
                 `McpDataContext as dyn McpToolContext`, a dataloader batch through \
                 `LoaderScope as dyn GraphqlBatchContext`, and worker/cron jobs through \
                 SeaOrmDatabaseModule's JobContext — none of those is bound on this path";
            tracing::error!(
                target: crate::TARGET,
                entity = std::any::type_name::<E>(),
                hint = HINT,
                "no ambient database executor",
            );
            DbErr::Custom(format!("no ambient database executor — {HINT}"))
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

    /// A [`Select`] that **bypasses the ambient ability filter**, for the three
    /// sanctioned ability-less reads only; every other read uses
    /// [`scoped`](Self::scoped):
    ///
    /// 1. **Pre-authentication** credential lookup, before any principal exists.
    /// 2. **Access binding** (`CrudService::access`), which must tell `Denied`
    ///    from `Missing` and applies the ability check itself.
    /// 3. **Global uniqueness probes**
    ///    ([`resolve_unique_slug`](crate::resolve_unique_slug)), over rows the
    ///    caller cannot see too.
    ///
    /// **Not** a signature-authenticated webhook: none exists, and one first
    /// needs raw-body HMAC-SHA256 verification in a `Guard`, a constant-time
    /// compare, a replay window, fail-closed secret handling and a denial `warn`.
    pub fn unscoped() -> Select<E> {
        E::find()
    }

    /// The by-primary-key analog of [`unscoped`](Self::unscoped), under the same
    /// bar.
    pub fn unscoped_by_id(id: <E::PrimaryKey as PrimaryKeyTrait>::ValueType) -> Select<E> {
        E::find_by_id(id)
    }

    /// System **write** — the write pendant of [`unscoped`](Self::unscoped): a
    /// raw insert with no ability filter, on an explicit connection.
    ///
    /// Bar: **pre-principal provisioning** (a social-login user) and
    /// principal-less system work. Every authorized create goes through
    /// [`Creatable::create_from_active`](crate::Creatable).
    pub async fn insert_unscoped<C>(active: E::ActiveModel, conn: &C) -> Result<E::Model, DbErr>
    where
        C: ConnectionTrait,
        E::ActiveModel: ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        tracing::trace!(
            target: crate::TARGET,
            entity = E::default().table_name(),
            "insert unscoped",
        );
        active.insert(conn).await
    }

    /// Update a row, gated by `condition_for(Update)` ANDed with the primary
    /// key: a row outside the caller's scope is never touched and surfaces as
    /// [`DbErr::RecordNotUpdated`], so a caller cannot mutate by id past its
    /// scope.
    ///
    /// Runs [`ActiveModelBehavior`] — see `scoped_save`.
    pub async fn update<A>(active: A) -> Result<E::Model, DbErr>
    where
        A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<A>,
    {
        Self::scoped_save(active, Action::Update).await
    }

    /// The scope filter forces sea-orm's `Update::one`, which does not run
    /// [`ActiveModelBehavior`]: the hooks are driven here, or `updated_at` never
    /// moves.
    async fn scoped_save<A>(active: A, action: Action) -> Result<E::Model, DbErr>
    where
        A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<A>,
    {
        let conn = Self::conn()?;
        let active = A::before_save(active, &conn, false).await?;
        let model = Update::one(active)
            .validate()?
            .filter(scope_for::<E>(action))
            .exec(&conn)
            .await?;
        A::after_save(model, &conn, false).await
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
        A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send,
        M: IntoActiveModel<A> + Send,
        E::Model: IntoActiveModel<A>,
    {
        let mut active = model.into_active_model();
        active.set(col, Value::from(crate::now()));
        Self::scoped_save(active, Action::Delete).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_authz::{AbilityBuilder, with_ability};
    use sea_orm::sea_query::{Condition, Expr};
    use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait, Set};

    use super::*;
    use crate::executor::{Executor, with_executor, with_job_executor, with_request_executor};
    use crate::soft_delete::live_condition;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod tombstone {
        use sea_orm::entity::prelude::*;

        use crate::soft_delete::SoftDeletable;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "tombstones")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub deleted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

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

    #[tokio::test]
    async fn a_missing_executor_is_logged_with_the_binding_that_would_supply_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let Err(err) = Repo::<widget::Entity>::conn() else {
            panic!("no executor is installed on this task");
        };

        let detail = err.to_string();
        assert!(detail.contains("no ambient database executor"), "{detail}");
        assert!(
            detail.contains("LoaderScope as dyn GraphqlBatchContext"),
            "the dataloader binding is one of the ones named: {detail}",
        );

        let event = logs.expect_one(crate::TARGET, "no ambient database executor");
        assert_eq!(event.level, "error");
        assert!(
            event.field("entity").is_some_and(|e| e.contains("widget")),
            "…against the entity that could not be read: {event:#?}",
        );
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("LoaderScope as dyn GraphqlBatchContext")),
            "…and the bindings that would supply one: {event:#?}",
        );
    }

    #[tokio::test]
    async fn conn_without_ambient_executor_is_an_error() {
        match Repo::<widget::Entity>::conn() {
            Ok(_) => panic!("no ambient executor must error"),
            Err(err) => assert!(
                err.to_string().contains("no ambient database executor"),
                "the error names the missing scope: {err}",
            ),
        }
    }

    #[tokio::test]
    async fn no_ambient_state_denies_by_default() {
        let s = sql(scope_for::<widget::Entity>(Action::Read));
        assert!(s.contains("1 = 0"), "untagged ⇒ deny-all: {s}");
    }

    #[tokio::test]
    async fn untagged_executor_without_ability_denies_all_rows() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_executor(pool, async {
            assert_eq!(current_executor_scope(), None);
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(s.contains("1 = 0"), "untagged executor fails closed: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn request_scope_without_ability_denies_all_rows() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_request_executor(pool, async {
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(s.contains("1 = 0"), "request paths fail closed: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn denying_all_rows_is_loud() {
        let logs = nest_rs_testing::LogCapture::install();
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_request_executor(pool, async {
            let _ = scope_for::<widget::Entity>(Action::Read);
        })
        .await;

        let event = logs.expect_one(
            crate::TARGET,
            "no ambient Ability outside a worker job — denying all rows",
        );
        assert_eq!(event.level, "warn");
        assert!(
            event.field("entity").is_some_and(|e| e.contains("widget")),
            "the entity is named: {event:#?}",
        );
        assert_eq!(event.field("action").as_deref(), Some("Read"));
    }

    #[tokio::test]
    async fn the_unscoped_job_path_is_silent() {
        let logs = nest_rs_testing::LogCapture::install();
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_job_executor(pool, async {
            let _ = scope_for::<widget::Entity>(Action::Read);
        })
        .await;
        logs.expect_none(
            crate::TARGET,
            "no ambient Ability outside a worker job — denying all rows",
        );
    }

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
        let ability = Arc::new(b.build().expect("valid test ability"));

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

    #[test]
    fn repo_conn_outside_scope_names_the_interceptor() {
        let msg = match Repo::<widget::Entity>::conn() {
            Ok(_) => panic!("expected error outside the executor scope"),
            Err(err) => err.to_string(),
        };
        assert!(msg.contains("ambient"), "missing 'ambient': {msg}");
        assert!(msg.contains("DbContext"), "missing 'DbContext': {msg}");
    }

    #[test]
    fn deny_all_condition_serializes_as_one_equals_zero() {
        let s = sql(Condition::all().add(Expr::cust("1 = 0")));
        assert!(s.contains("1 = 0"), "got: {s}");
    }

    fn select_sql(select: Select<widget::Entity>) -> String {
        select.build(DatabaseBackend::Postgres).to_string()
    }

    #[tokio::test]
    async fn scoped_denies_by_default_outside_a_job() {
        let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
        assert!(s.contains("1 = 0"), "untagged ⇒ deny-all: {s}");
    }

    #[tokio::test]
    async fn scoped_in_request_without_ability_renders_deny_all() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        with_request_executor(pool, async {
            let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
            assert!(s.contains("1 = 0"), "scoped() fails closed too: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn scoped_with_unconditional_grant_renders_unrestricted() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
                assert!(!s.contains("1 = 0"), "admin should not be denied: {s}");
            })
            .await;
        })
        .await;
    }

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
        let ability = Arc::new(b.build().expect("valid test ability"));

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

                assert_ne!(read, create);
                assert_ne!(read, update);
                assert_ne!(read, delete);
                assert_ne!(create, update);
            })
            .await;
        })
        .await;
    }

    #[tokio::test]
    async fn scope_for_denies_an_unmentioned_action_inside_request() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let s = sql(scope_for::<widget::Entity>(Action::Delete));
                assert!(s.contains("1 = 0"), "missing action denies: {s}");
            })
            .await;
        })
        .await;
    }

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

    #[tokio::test]
    async fn update_by_id_ands_the_pk_with_the_ability_scope() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 91));
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let mut active = widget::ActiveModel {
                    id: Set(424242),
                    ..Default::default()
                };
                active.name = Set("hacked".to_owned());
                let s = Update::one(active)
                    .validate()
                    .expect("a PK-bearing active model validates")
                    .filter(scope_for::<widget::Entity>(Action::Update))
                    .build(DatabaseBackend::Postgres)
                    .to_string();

                assert!(s.contains("424242"), "the PK is in the WHERE: {s}");
                assert!(
                    s.contains("org_id"),
                    "the ability scope is in the WHERE: {s}"
                );
                assert!(
                    s.contains("91"),
                    "the ability predicate value is applied: {s}"
                );
                assert!(
                    s.contains("AND"),
                    "the PK and the ability scope are ANDed: {s}"
                );
                assert!(!s.contains("1 = 0"), "a granted action is not denied: {s}");
            })
            .await;
        })
        .await;
    }

    #[tokio::test]
    async fn delete_by_id_ands_the_pk_with_the_ability_scope() {
        let pool = Executor::Pool(sea_orm::DatabaseConnection::default());
        let mut b = AbilityBuilder::new();
        b.can(Action::Delete, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 91));
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            with_ability(ability, async {
                let active = widget::ActiveModel {
                    id: Set(424242),
                    ..Default::default()
                };
                let s = Delete::one(active)
                    .validate()
                    .expect("a PK-bearing active model validates")
                    .filter(scope_for::<widget::Entity>(Action::Delete))
                    .build(DatabaseBackend::Postgres)
                    .to_string();

                assert!(s.contains("424242"), "the PK is in the WHERE: {s}");
                assert!(
                    s.contains("org_id"),
                    "the ability scope is in the WHERE: {s}"
                );
                assert!(
                    s.contains("91"),
                    "the ability predicate value is applied: {s}"
                );
                assert!(
                    s.contains("AND"),
                    "the PK and the ability scope are ANDed: {s}"
                );
                assert!(!s.contains("1 = 0"), "a granted action is not denied: {s}");
            })
            .await;
        })
        .await;
    }
}
