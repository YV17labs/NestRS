use std::any::TypeId;
use std::marker::PhantomData;

use nest_rs_authz::__private::visitors_read_unnamed;
use nest_rs_authz::{Action, Predicate, current_ability};
use nest_rs_database::{ExecutorScope, atomic, current_executor_scope};
use sea_orm::sea_query::{Condition, Expr, ExprTrait, FromValueTuple, OnConflict};
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ActiveValue, ColumnTrait, DbErr, Delete, DeleteResult,
    EntityName, EntityTrait, IdenStatic, Insert, IntoActiveModel, Iterable, ModelTrait,
    PrimaryKeyToColumn, PrimaryKeyTrait, QueryFilter, QueryTrait, Select, Update, Value,
};

use crate::column_default::fill_column_defaults;
use crate::contained::contained_values;
use crate::error::{AtomicDbError, write_refused};
use crate::executor::{SeaOrmExecutor, current_executor};
use crate::insertion::Insertion;
use crate::select::RepoSelect;
use crate::soft_delete::{live_condition_of, tombstone_column};

/// Row-level filter for `action` on `E`: the ambient ability's condition, and,
/// for every action but [`Create`](Action::Create) and
/// [`Delete`](Action::Delete), the one hiding the tombstones of an entity
/// declared `#[expose(..., soft_delete)]`. A worker/system (`Job`) executor is
/// the sole path without an ability that the ability leaves unscoped; every
/// other one — request-scoped or untagged — denies all rows (fail-closed).
///
/// `Create` checks a fresh row against the ability alone, so a row inserted
/// already deleted (an archive import) is no denial. `Delete` reaches a
/// tombstoned row: [`Repo::delete`] is the hard purge, and
/// [`Repo::soft_delete`] stays idempotent.
pub fn scope_for<E: EntityTrait>(action: Action) -> Condition {
    live_within::<E>(action, ability_scope::<E>(action))
}

/// `scope`, hiding the tombstones of a soft-deletable `E` from every action but
/// `Create` and `Delete` ([`scope_for`]).
fn live_within<E: EntityTrait>(action: Action, scope: Condition) -> Condition {
    if matches!(action, Action::Create | Action::Delete) {
        return scope;
    }
    match live_condition_of::<E>() {
        Some(live) => Condition::all().add(scope).add(live),
        None => scope,
    }
}

/// The ambient ability's condition for `action` on `E`, alone.
pub(crate) fn ability_scope<E: EntityTrait>(action: Action) -> Condition {
    match current_ability() {
        Some(ability) => ability.condition_for::<E>(action),
        None if current_executor_scope() == Some(ExecutorScope::Job) => Condition::all(),
        None => {
            denying_for_want_of_an_ability::<E>(action);
            Predicate::<E>::Deny.to_condition()
        }
    }
}

/// What an `Update` or a `Create` holds a write to: [`scope_for`], or, under a grant
/// whose `.fields` narrows what its action writes, the rows on which the grants
/// permit each column `active` chooses
/// ([`condition_for_columns`](nest_rs_authz::Ability::condition_for_columns)),
/// refused by name when none does, which sends nothing. Read before a hook, a pin or
/// a default fills `active`: what they set is the server's. A job holds no ability
/// and no column is withheld from it.
pub(crate) fn write_scope<A>(action: Action, active: &A) -> Result<Condition, DbErr>
where
    A: ActiveModelTrait,
{
    let subject = TypeId::of::<A::Entity>();
    let Some(ability) =
        current_ability().filter(|ability| !ability.grants_every_column(action, subject))
    else {
        return Ok(scope_for::<A::Entity>(action));
    };
    let columns = chosen_columns(action, active);
    match ability.condition_for_columns::<A::Entity>(action, &columns) {
        Ok(scope) => Ok(live_within::<A::Entity>(action, scope)),
        Err(refused) => {
            let table = A::Entity::default().table_name();
            tracing::warn!(
                target: crate::target::ORM,
                entity = table,
                action = action.as_str(),
                columns = refused.columns.join(","),
                "access denied — a column the caller's grants withhold",
            );
            let statement = match action {
                Action::Create => "insert",
                _ => "update",
            };
            Err(write_refused(statement, table, &refused.columns))
        }
    }
}

/// The columns a write chooses. Never the key, which addresses the row on an update
/// and which a `Uuid` input seeds on a create: a grant's `when` holds it. A null on a
/// create is no choice either, an input lowering a member it did not carry to one.
fn chosen_columns<A: ActiveModelTrait>(
    action: Action,
    active: &A,
) -> Vec<<A::Entity as EntityTrait>::Column> {
    <A::Entity as EntityTrait>::Column::iter()
        .filter(|&column| {
            <A::Entity as EntityTrait>::PrimaryKey::from_column(column).is_none()
                && match active.get(column) {
                    ActiveValue::Set(value) => action != Action::Create || value != value.as_null(),
                    ActiveValue::Unchanged(_) | ActiveValue::NotSet => false,
                }
        })
        .collect()
}

/// Whether no row of `E` can pass `action`'s scope, known without a statement:
/// the ability bars the class — no grant, or a denial no `when` narrows — or
/// none is ambient outside a job.
pub(crate) fn barred_from_class<E: EntityTrait>(action: Action) -> bool {
    match current_ability() {
        Some(ability) => !ability.can_class(action, TypeId::of::<E>()),
        None if current_executor_scope() == Some(ExecutorScope::Job) => false,
        None => {
            denying_for_want_of_an_ability::<E>(action);
            true
        }
    }
}

/// The route answers `200 []`, or the write `RecordNotUpdated`, which reads as a
/// business bug: this event is the only place the cause shows.
fn denying_for_want_of_an_ability<E: EntityTrait>(action: Action) {
    if visitors_read_unnamed() {
        tracing::warn!(
            target: crate::target::ORM,
            entity = E::default().table_name(),
            action = action.as_str(),
            scope = current_executor_scope().map(ExecutorScope::as_str),
            hint = "a visitor reads only inside the unit naming its read, #[public(Read, Entity)]: \
                    a nested GraphQL field or a relation's batch names none and takes no posture, \
                    so read it in that unit, or answer it to a caller a token backs",
            "a visitor's read outside the unit naming it — denying all rows",
        );
        return;
    }
    tracing::warn!(
        target: crate::target::ORM,
        entity = E::default().table_name(),
        action = action.as_str(),
        scope = current_executor_scope().map(ExecutorScope::as_str),
        hint = "bind #[use_guards(AuthnGuard, AuthzGuard)], and declare what this unit reads: \
                #[authorize(Read, Entity)], or #[public(Read, Entity)] where a caller with no \
                credential is admitted; a websocket connection hook declares none, so read it \
                in a message",
        "no ambient Ability outside a worker job — denying all rows",
    );
}

/// The key a host whose generated code runs `Repo` requires — a `#[crud]`
/// host, a route or a field loading a bound subject, the service an exposed
/// entity names for its loaders — with its label: the unit of work its
/// executor comes from, so a host reached with no binding fails the boot
/// instead of every call.
pub fn unit_of_work_required() -> (std::any::TypeId, &'static str) {
    (
        std::any::TypeId::of::<std::sync::Arc<dyn nest_rs_database::UnitOfWork>>(),
        "dyn UnitOfWork",
    )
}

/// Repository over entity `E`, bound to the ambient request executor and ability.
///
/// It hands out no connection and no statement: every statement it runs carries
/// the ambient ability's filter ([`RepoSelect`]), or goes through an escape named
/// for its bar. A query of a service's own on the unit's executor does not compile:
///
/// ```compile_fail,E0624
/// # mod widget {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "widgets")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::EntityTrait;
/// # async fn every_row() -> Result<(), sea_orm::DbErr> {
/// let unfiltered = widget::Entity::find()
///     .all(&Repo::<widget::Entity>::conn()?)
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct Repo<E: EntityTrait>(PhantomData<fn() -> E>);

impl<E: EntityTrait> Repo<E> {
    /// The ambient executor (transaction when open, else the pool) every `Repo`
    /// statement runs on, or the error naming the bindings that install one.
    pub(crate) fn conn() -> Result<SeaOrmExecutor, DbErr> {
        current_executor().ok_or_else(|| {
            // Logged: on the wire it is the opaque `INTERNAL` answer.
            const HINT: &str = "a Repo query runs against the executor of the unit of work its \
                 edge runs it in — an HTTP request, a WS message or hook, an MCP operation, a \
                 queue attempt, a scheduled tick, a listener call, a lifecycle hook — which each \
                 opens through the bound `dyn UnitOfWork`, SeaOrmDatabaseModule's; a dataloader \
                 batch, a streaming body and after-commit work run on the pool of the unit they \
                 continue, and a task spawned off a unit carries none, so it opens its own — no \
                 unit of work runs on this path";
            tracing::error!(
                target: crate::target::ORM,
                entity = E::default().table_name(),
                hint = HINT,
                "no ambient database executor",
            );
            DbErr::Custom(format!("no ambient database executor — {HINT}"))
        })
    }

    /// Every row of `E` the caller may [`Read`](Action::Read), soft-deleted rows
    /// left out.
    pub async fn all() -> Result<Vec<E::Model>, DbErr> {
        Self::scoped(Action::Read).all().await
    }

    /// A row by primary key, returned only if the caller may
    /// [`Read`](Action::Read) it. A row outside the caller's scope reads as
    /// `None` — never leaking its existence.
    pub async fn find_by_id(
        id: <E::PrimaryKey as PrimaryKeyTrait>::ValueType,
    ) -> Result<Option<E::Model>, DbErr> {
        RepoSelect::new(E::find_by_id(id).filter(scope_for::<E>(Action::Read)))
            .one()
            .await
    }

    /// The rows the caller may `action` ([`scope_for`]), for a custom query:
    /// chain the select's filters, orders and limits, then run it on the ambient
    /// executor.
    pub fn scoped(action: Action) -> RepoSelect<E> {
        RepoSelect::new(E::find().filter(scope_for::<E>(action)))
    }

    /// The rows the caller may `action` **with their soft-deleted rows**, which
    /// every other read hides: the one way to read a tombstone. In a job it
    /// reads every row, as [`scoped`](Self::scoped) does.
    ///
    /// Bar: the deleted row is what the caller is after — an administrator's
    /// view of deleted records, a retention job — never a read that means to
    /// show live rows. It reads a tombstone only: no `Repo` write un-deletes
    /// one, since [`update`](Self::update) hides it too; a restore write is
    /// planned for 7.x.
    pub fn with_tombstones(action: Action) -> RepoSelect<E> {
        RepoSelect::new(E::find().filter(ability_scope::<E>(action)))
    }

    /// Whether the caller may `action` this row, decided in SQL: one query on
    /// its primary key under the action's own condition, which decides a
    /// relational rule [`Ability::can`](nest_rs_authz::Ability::can) cannot
    /// see on the model alone.
    ///
    /// The instance check for a model read for another action — `Update` on a
    /// row read for `Read`. A row gone since it was read answers `false`, and
    /// so does every row when the scope denies all of them
    /// ([`scope_for`]).
    pub async fn can(action: Action, model: &E::Model) -> Result<bool, DbErr> {
        let allowed = Self::scoped(action)
            .within(pk_condition::<E>(model))
            .exists()
            .await?;
        tracing::trace!(
            target: crate::target::ORM,
            entity = E::default().table_name(),
            action = action.as_str(),
            allowed,
            "instance check",
        );
        Ok(allowed)
    }

    /// The rows of `E` **with no ambient ability filter**, for the three
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
    ///
    /// Soft-deleted rows stay hidden: hiding them is no authorization filter,
    /// and a deleted account must not sign in. It runs on the ambient executor
    /// like every other read.
    pub fn unscoped() -> RepoSelect<E> {
        RepoSelect::new(live_rows(E::find()))
    }

    /// The by-primary-key analog of [`unscoped`](Self::unscoped), under the same
    /// bar.
    pub fn unscoped_by_id(id: <E::PrimaryKey as PrimaryKeyTrait>::ValueType) -> RepoSelect<E> {
        RepoSelect::new(live_rows(E::find_by_id(id)))
    }

    /// System **write** — the write pendant of [`unscoped`](Self::unscoped): an
    /// insert with no ability filter, on the ambient executor.
    ///
    /// Bar: **pre-principal provisioning** (a social-login user) and
    /// principal-less system work. Every authorized create goes through
    /// [`Creatable::create_from_active`](crate::Creatable).
    ///
    /// A column the row leaves unset takes the default its `#[expose]` entity
    /// declares: a v7 UUID for a `Uuid` key, its `#[expose(default = …)]` for
    /// any other.
    pub async fn insert_unscoped(mut active: E::ActiveModel) -> Result<E::Model, DbErr>
    where
        E::ActiveModel: ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        let conn = Self::conn()?;
        fill_column_defaults::<E>(&mut active);
        tracing::trace!(
            target: crate::target::ORM,
            entity = E::default().table_name(),
            "insert unscoped",
        );
        insert_contained::<E>(active, &conn).await
    }

    /// [`insert_unscoped`](Self::insert_unscoped), unless a live row already
    /// holds the row's values of the unique `key`: then nothing is written and
    /// that row is returned, [`Insertion::Existing`]. The conflict is absorbed
    /// by the statement itself — `INSERT … ON CONFLICT (key) DO NOTHING
    /// RETURNING`, then a read of the row holding the key — so it never aborts
    /// the unit's transaction, and no savepoint wraps it.
    ///
    /// Bar: [`insert_unscoped`](Self::insert_unscoped)'s — pre-principal
    /// provisioning and principal-less system work — where a concurrent
    /// caller may have inserted the same key first (a social login's user and
    /// its identity link). The key needs a unique index over exactly its
    /// columns; on an entity declared `soft_delete`, one over its live rows
    /// alone (`WHERE deleted_at IS NULL`) is inferred too, and a tombstone is
    /// never returned.
    ///
    /// A key column the row leaves unset, or its `before_save` unsets, is refused
    /// before anything is sent ([`DbErr::AttrNotSet`]), and the row holding the
    /// key is read by what that hook returned, the values the insert sends. When
    /// no live row holds the key after all — a
    /// tombstone a unique index counts holds it, or the row was deleted since
    /// — the insert is sent again inside a savepoint, so its own outcome
    /// stands: the row, or the violation it meets
    /// ([`DbOutcome::Conflict`](crate::DbOutcome::Conflict)).
    ///
    /// ```
    /// # mod identity {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    /// #     #[sea_orm(table_name = "user_identity")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key, auto_increment = false)]
    /// #         pub id: Uuid,
    /// #         pub provider: String,
    /// #         pub subject: String,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// use nest_rs_seaorm::{Insertion, Repo};
    /// use sea_orm::{DbErr, Set};
    ///
    /// async fn link(provider: &str, subject: &str) -> Result<bool, DbErr> {
    ///     let link = identity::ActiveModel {
    ///         provider: Set(provider.to_owned()),
    ///         subject: Set(subject.to_owned()),
    ///         ..Default::default()
    ///     };
    ///     let key = [identity::Column::Provider, identity::Column::Subject];
    ///     match Repo::<identity::Entity>::insert_unscoped_or_existing(link, key).await? {
    ///         Insertion::New(_) => Ok(true),
    ///         Insertion::Existing(_) => Ok(false),
    ///     }
    /// }
    /// ```
    pub async fn insert_unscoped_or_existing<const N: usize>(
        mut active: E::ActiveModel,
        key: [E::Column; N],
    ) -> Result<Insertion<E::Model>, DbErr>
    where
        E::ActiveModel: ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        const {
            assert!(
                N > 0,
                "an on-conflict insert names the unique key it may meet"
            )
        };
        let conn = Self::conn()?;
        fill_column_defaults::<E>(&mut active);
        holding_key_of::<E>(&active, key)?;
        let active = E::ActiveModel::before_save(active, &conn, true).await?;
        contained_values(&active)?;
        let held = holding_key_of::<E>(&active, key)?;
        let entity = E::default().table_name();
        // sea-orm 2.0's single-row `exec_with_returning`, `TryInsert`'s included,
        // answers `RecordNotFound` to an empty `RETURNING`: only the list form
        // tells the met key from a failure.
        let inserted = absorbing_conflicts_on::<E>(active.clone(), key)
            .exec_with_returning_many(&conn)
            .await?;
        let model = match inserted.into_iter().next() {
            Some(model) => model,
            None => {
                if let Some(existing) = Self::unscoped().within(held).one().await? {
                    tracing::trace!(target: crate::target::ORM, entity, "insert unscoped met its key");
                    return Ok(Insertion::Existing(existing));
                }
                atomic(async move {
                    let conn = Self::conn()?;
                    Ok::<_, AtomicDbError>(E::insert(active).exec_with_returning(&conn).await?)
                })
                .await
                .map_err(DbErr::from)?
            }
        };
        tracing::trace!(target: crate::target::ORM, entity, "insert unscoped");
        E::ActiveModel::after_save(model, &conn, true)
            .await
            .map(Insertion::New)
    }

    /// Update a row, gated by `condition_for(Update)` ANDed with the primary
    /// key: a row outside the caller's scope, or soft-deleted, is never touched
    /// and surfaces as [`DbErr::RecordNotUpdated`], so a caller cannot mutate by
    /// id past its scope. An `active` setting nothing sends no `UPDATE` and hands
    /// back the row under that same scope, or the same error.
    ///
    /// Runs [`ActiveModelBehavior`] — see `scoped_save` — but for a caller no
    /// row could pass for (no grant, a denial no `when` narrows, or a request
    /// holding no ability, as on a route whose posture names no subject), who
    /// gets the same error before any hook runs.
    ///
    /// A grant's `.fields` limits what `active` sets, its hook's stamps aside: a
    /// column no grant permits is [`DbErr::AccessDenied`] naming it, before any hook
    /// runs or anything is sent, and a row whose grants do not permit each column is
    /// outside the scope.
    pub async fn update<A>(active: A) -> Result<E::Model, DbErr>
    where
        A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<A>,
    {
        Self::scoped_save(active, Action::Update).await
    }

    /// The scope filter forces sea-orm's `Update::one`, which does not run
    /// [`ActiveModelBehavior`]: the hooks are driven here, or `updated_at` never
    /// moves. A caller no row could pass for (see [`barred_from_class`]) gets
    /// what a row outside its scope gets, before any hook runs; a job, which
    /// holds no ability, is unscoped.
    async fn scoped_save<A>(active: A, action: Action) -> Result<E::Model, DbErr>
    where
        A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send,
        E::Model: IntoActiveModel<A>,
    {
        let conn = Self::conn()?;
        if barred_from_class::<E>(action) {
            return Err(DbErr::RecordNotUpdated);
        }
        // A soft delete's stamp is the framework's, whatever a grant's `.fields` say.
        let scope = match action {
            Action::Update => write_scope(action, &active)?,
            _ => scope_for::<E>(action),
        };
        let active = A::before_save(active, &conn, false).await?;
        contained_values(&active)?;
        let key = active.get_primary_key_value();
        let update = Update::one(active).validate()?;
        // Setting nothing, sea-orm sends no `UPDATE` and reads the row back by its key
        // alone, so the scope, on a `WHERE` never sent, would hold nothing.
        let model = if update.as_query().get_values().is_empty() {
            let key = key.ok_or(DbErr::UpdateGetPrimaryKey)?;
            E::find_by_id(<E::PrimaryKey as PrimaryKeyTrait>::ValueType::from_value_tuple(key))
                .filter(scope)
                .one(&conn)
                .await?
                .ok_or(DbErr::RecordNotUpdated)?
        } else {
            update.filter(scope).exec(&conn).await?
        };
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
    /// A caller no row could pass for gets [`DbErr::RecordNotUpdated`] before any
    /// hook runs, as [`update`](Self::update).
    ///
    /// The row is then hidden from every read but
    /// [`with_tombstones`](Self::with_tombstones) when `E` is declared
    /// `#[expose(..., soft_delete)]` and `col` is its `deleted_at`.
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

/// The rows holding `active`'s values of `key`, or the column it leaves unset.
fn holding_key_of<E: EntityTrait>(
    active: &E::ActiveModel,
    key: impl IntoIterator<Item = E::Column>,
) -> Result<Condition, DbErr> {
    key.into_iter().try_fold(Condition::all(), |held, column| {
        let value = active
            .get(column)
            .into_value()
            .ok_or_else(|| DbErr::AttrNotSet(column.as_str().to_owned()))?;
        Ok(held.add(column.eq(value)))
    })
}

/// `active`'s insert, doing nothing on a conflict over `key` — over the live
/// rows of a soft-deletable `E`, so a partial index over them is inferred.
fn absorbing_conflicts_on<E: EntityTrait>(
    active: E::ActiveModel,
    key: impl IntoIterator<Item = E::Column>,
) -> Insert<E::ActiveModel> {
    let mut on_conflict = OnConflict::columns(key);
    on_conflict.do_nothing();
    if let Some(tombstone) = tombstone_column::<E>() {
        on_conflict.target_and_where(Expr::col(tombstone).is_null());
    }
    E::insert(active).on_conflict(on_conflict)
}

/// `active`'s insert, its hooks run around it as sea-orm's `insert` runs them, refused
/// before any statement when a value its hook leaves would not stay bound.
pub(crate) async fn insert_contained<E>(
    active: E::ActiveModel,
    conn: &SeaOrmExecutor,
) -> Result<E::Model, DbErr>
where
    E: EntityTrait,
    E::ActiveModel: ActiveModelBehavior + Send,
    E::Model: IntoActiveModel<E::ActiveModel>,
{
    let active = E::ActiveModel::before_save(active, conn, true).await?;
    contained_values(&active)?;
    let model = E::insert(active).exec_with_returning(conn).await?;
    E::ActiveModel::after_save(model, conn, true).await
}

/// `select`, without the tombstones of a soft-deletable `E`.
fn live_rows<E: EntityTrait>(select: Select<E>) -> Select<E> {
    match live_condition_of::<E>() {
        Some(live) => select.filter(live),
        None => select,
    }
}

/// Equality over **all** of a model's primary-key columns, composite included.
pub(crate) fn pk_condition<E: EntityTrait>(model: &E::Model) -> Condition {
    let mut cond = Condition::all();
    for pk in E::PrimaryKey::iter() {
        let col = pk.into_column();
        cond = cond.add(col.eq(model.get(col)));
    }
    cond
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_authz::__private::install_ability;
    use nest_rs_authz::AbilityBuilder;
    use nest_rs_database::__private::{with_executor, with_job_executor, with_request_executor};
    use nest_rs_database::Executor;
    use sea_orm::sea_query::{Condition, Expr};
    use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait, Set};

    use super::*;
    use crate::soft_delete::live_condition;

    /// A pool handle that dials nothing, as the port installs it.
    fn pool() -> Arc<dyn Executor> {
        SeaOrmExecutor::Pool(sea_orm::DatabaseConnection::default()).into()
    }

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

        use crate::soft_delete::{SoftDeletable, SoftDeleteDeclared};

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

        impl SoftDeleteDeclared for Entity {}

        impl SoftDeletable for Entity {
            fn deleted_at_column() -> Column {
                Column::DeletedAt
            }
        }
    }

    inventory::submit! {
        crate::soft_delete::SoftDeleteRegistration {
            entity: || std::any::TypeId::of::<tombstone::Entity>(),
            column: || sea_orm::IdenStatic::as_str(&tombstone::Column::DeletedAt),
        }
    }

    const LIVE: &str = r#""tombstones"."deleted_at" IS NULL"#;

    fn tombstone_sql(select: RepoSelect<tombstone::Entity>) -> String {
        select.build(DatabaseBackend::Postgres).to_string()
    }

    #[tokio::test]
    async fn every_action_but_create_and_delete_hides_a_soft_deletable_entitys_tombstones() {
        let pool = pool();
        with_job_executor(pool, async {
            for action in [Action::Read, Action::Update, Action::Manage] {
                let s = tombstone_sql(Repo::<tombstone::Entity>::scoped(action));
                assert!(s.contains(LIVE), "{action:?} hides tombstones: {s}");
            }
            let s = tombstone_sql(Repo::<tombstone::Entity>::scoped(Action::Create));
            assert!(
                !s.contains("IS NULL"),
                "a row inserted already deleted (an archive import) passes the create check: {s}",
            );
            let s = tombstone_sql(Repo::<tombstone::Entity>::scoped(Action::Delete));
            assert!(
                !s.contains("IS NULL"),
                "the hard purge and the soft delete reach a tombstoned row: {s}",
            );
        })
        .await;
    }

    #[tokio::test]
    async fn an_entity_without_tombstones_carries_no_live_condition() {
        let pool = pool();
        with_job_executor(pool, async {
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(!s.contains("deleted_at"), "{s}");
        })
        .await;
    }

    #[tokio::test]
    async fn the_live_condition_is_anded_after_the_abilitys() {
        let pool = pool();
        with_request_executor(pool, async {
            let s = tombstone_sql(Repo::<tombstone::Entity>::scoped(Action::Read));
            assert!(
                s.contains("1 = 0") && s.contains(LIVE),
                "hiding tombstones never widens a denial: {s}",
            );
        })
        .await;
    }

    #[test]
    fn the_ability_less_escapes_hide_tombstones_too() {
        let s = tombstone_sql(Repo::<tombstone::Entity>::unscoped());
        assert!(s.contains(LIVE), "unscoped: {s}");
        let s = tombstone_sql(Repo::<tombstone::Entity>::unscoped_by_id(7));
        assert!(s.contains(LIVE) && s.contains('7'), "unscoped_by_id: {s}");
    }

    #[tokio::test]
    async fn with_tombstones_keeps_the_ability_and_drops_the_live_condition() {
        let pool = pool();
        with_job_executor(pool.clone(), async {
            let s = tombstone_sql(Repo::<tombstone::Entity>::with_tombstones(Action::Read));
            assert!(!s.contains("IS NULL"), "a job reads every row: {s}");
        })
        .await;
        with_request_executor(pool, async {
            let s = tombstone_sql(Repo::<tombstone::Entity>::with_tombstones(Action::Read));
            assert!(s.contains("1 = 0"), "still scoped by the ability: {s}");
            assert!(!s.contains("IS NULL"), "{s}");
        })
        .await;
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
            detail.contains("SeaOrmDatabaseModule"),
            "the binding every edge opens its units through is named: {detail}",
        );

        let event = logs.expect_one(crate::target::ORM, "no ambient database executor");
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("entity").as_deref(),
            Some("widgets"),
            "…against the entity that could not be read: {event:#?}",
        );
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("a task spawned off a unit carries none")),
            "…and the bindings that would supply one: {event:#?}",
        );
    }

    #[tokio::test]
    async fn a_task_spawned_off_a_unit_fails_closed_for_want_of_an_executor() {
        let ability = Arc::new(AbilityBuilder::new().build().expect("an empty ability"));
        let pool = pool();
        let carried = with_request_executor(
            pool,
            install_ability(ability, async {
                assert!(Repo::<widget::Entity>::conn().is_ok(), "the unit has one");
                nest_rs_core::TaskContext::current()
            }),
        )
        .await;

        let (opened, scoped) = tokio::spawn(carried.carry(async {
            (
                Repo::<widget::Entity>::conn().is_ok(),
                nest_rs_authz::current_ability().is_some(),
            )
        }))
        .await
        .expect("the task runs");
        assert!(
            !opened,
            "a task that may outlive its unit opens its own, never borrows its spawner's",
        );
        assert!(scoped, "the caller's ability still travels with it");
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
        let pool = pool();
        with_executor(pool, async {
            assert_eq!(current_executor_scope(), None);
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(s.contains("1 = 0"), "untagged executor fails closed: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn request_scope_without_ability_denies_all_rows() {
        let pool = pool();
        with_request_executor(pool, async {
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(s.contains("1 = 0"), "request paths fail closed: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn denying_all_rows_is_loud() {
        let logs = nest_rs_testing::LogCapture::install();
        let pool = pool();
        with_request_executor(pool, async {
            let _ = scope_for::<widget::Entity>(Action::Read);
        })
        .await;

        let event = logs.expect_one(
            crate::target::ORM,
            "no ambient Ability outside a worker job — denying all rows",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("entity").as_deref(),
            Some("widgets"),
            "the table is named, as every other line of this target names it: {event:#?}",
        );
        assert_eq!(event.field("action").as_deref(), Some("read"));
        assert_eq!(event.field("scope").as_deref(), Some("request"));
    }

    #[tokio::test]
    async fn the_unscoped_job_path_is_silent() {
        let logs = nest_rs_testing::LogCapture::install();
        let pool = pool();
        with_job_executor(pool, async {
            let _ = scope_for::<widget::Entity>(Action::Read);
        })
        .await;
        logs.expect_none(
            crate::target::ORM,
            "no ambient Ability outside a worker job — denying all rows",
        );
    }

    #[tokio::test]
    async fn job_scope_without_ability_remains_unscoped() {
        let pool = pool();
        with_job_executor(pool, async {
            let s = sql(scope_for::<widget::Entity>(Action::Read));
            assert!(!s.contains("1 = 0"), "worker paths stay unscoped: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn an_ambient_ability_wins_over_the_scope_default() {
        let pool = pool();
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 7));
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            install_ability(ability, async {
                let s = sql(scope_for::<widget::Entity>(Action::Read));
                assert!(s.contains("org_id"), "ability condition is applied: {s}");
                assert!(!s.contains("1 = 0"));
            })
            .await;
        })
        .await;
    }

    #[test]
    fn repo_conn_outside_scope_names_the_port() {
        let msg = match Repo::<widget::Entity>::conn() {
            Ok(_) => panic!("expected error outside the executor scope"),
            Err(err) => err.to_string(),
        };
        assert!(msg.contains("ambient"), "missing 'ambient': {msg}");
        assert!(msg.contains("UnitOfWork"), "missing 'UnitOfWork': {msg}");
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "a constant holding no value: the clause this test reads"
    )]
    fn deny_all_condition_serializes_as_one_equals_zero() {
        let s = sql(Condition::all().add(Expr::cust("1 = 0")));
        assert!(s.contains("1 = 0"), "got: {s}");
    }

    fn select_sql(select: RepoSelect<widget::Entity>) -> String {
        select.build(DatabaseBackend::Postgres).to_string()
    }

    #[tokio::test]
    async fn scoped_denies_by_default_outside_a_job() {
        let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
        assert!(s.contains("1 = 0"), "untagged ⇒ deny-all: {s}");
    }

    #[tokio::test]
    async fn scoped_in_request_without_ability_renders_deny_all() {
        let pool = pool();
        with_request_executor(pool, async {
            let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
            assert!(s.contains("1 = 0"), "scoped() fails closed too: {s}");
        })
        .await;
    }

    #[tokio::test]
    async fn scoped_with_unconditional_grant_renders_unrestricted() {
        let pool = pool();
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            install_ability(ability, async {
                let s = select_sql(Repo::<widget::Entity>::scoped(Action::Read));
                assert!(!s.contains("1 = 0"), "admin should not be denied: {s}");
            })
            .await;
        })
        .await;
    }

    #[tokio::test]
    async fn scope_for_per_action_uses_distinct_predicates() {
        let pool = pool();
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
            install_ability(ability, async {
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
        let pool = pool();
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            install_ability(ability, async {
                let s = sql(scope_for::<widget::Entity>(Action::Delete));
                assert!(s.contains("1 = 0"), "missing action denies: {s}");
            })
            .await;
        })
        .await;
    }

    /// A visitor's ability reading the widgets of one org, and writing none.
    fn a_visitor_reading_one_org() -> Arc<nest_rs_authz::Ability> {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 1));
        Arc::new(b.build_visitor().expect("valid test ability"))
    }

    #[tokio::test]
    async fn a_job_narrowed_to_a_visitor_reads_and_writes_by_the_visitors_grant() {
        with_job_executor(
            pool(),
            nest_rs_authz::with_ability(a_visitor_reading_one_org(), async {
                let read = sql(scope_for::<widget::Entity>(Action::Read));
                assert!(read.contains("org_id"), "the visitor's rows: {read}");
                assert!(!barred_from_class::<widget::Entity>(Action::Read));
                assert!(
                    barred_from_class::<widget::Entity>(Action::Update),
                    "the visitor writes nothing, and a job reads as system work under none",
                );
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_request_unit_handed_a_visitor_by_the_harness_reads_by_its_grant() {
        with_request_executor(
            pool(),
            install_ability(a_visitor_reading_one_org(), async {
                let read = sql(scope_for::<widget::Entity>(Action::Read));
                assert!(read.contains("org_id"), "the visitor's rows: {read}");
                assert!(!read.contains("1 = 0"), "{read}");
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn repo_conn_returns_the_installed_request_executor() {
        let pool = pool();
        with_request_executor(pool, async {
            let conn = Repo::<widget::Entity>::conn().expect("an executor is installed");
            assert!(matches!(conn, SeaOrmExecutor::Pool(_)));
        })
        .await;
    }

    #[tokio::test]
    async fn repo_conn_returns_the_installed_job_executor() {
        let pool = pool();
        with_job_executor(pool, async {
            let conn = Repo::<widget::Entity>::conn().expect("an executor is installed");
            assert!(matches!(conn, SeaOrmExecutor::Pool(_)));
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

    /// The statement absorbs a conflict over its key alone, and over a
    /// soft-deletable entity's live rows, so a partial index over them is the
    /// one Postgres infers.
    #[test]
    fn an_on_conflict_insert_names_its_key_and_its_live_rows() {
        let tombstoned = absorbing_conflicts_on::<tombstone::Entity>(
            tombstone::ActiveModel {
                id: Set(7),
                ..Default::default()
            },
            [tombstone::Column::Id],
        )
        .build(DatabaseBackend::Postgres)
        .to_string();
        assert!(
            tombstoned.ends_with(r#"ON CONFLICT ("id") WHERE "deleted_at" IS NULL DO NOTHING"#),
            "{tombstoned}"
        );

        let plain = absorbing_conflicts_on::<widget::Entity>(
            widget::ActiveModel {
                id: Set(7),
                org_id: Set(1),
                name: Set("ada".to_owned()),
            },
            [widget::Column::OrgId, widget::Column::Name],
        )
        .build(DatabaseBackend::Postgres)
        .to_string();
        assert!(
            plain.ends_with(r#"ON CONFLICT ("org_id", "name") DO NOTHING"#),
            "{plain}"
        );
    }

    /// A key the row leaves unset could read back no row: refused before any
    /// hook runs or any statement is sent, on a pool that dials nothing.
    #[tokio::test]
    async fn an_on_conflict_insert_whose_key_is_unset_sends_nothing() {
        let refused = with_job_executor(pool(), async {
            Repo::<hooked::Entity>::insert_unscoped_or_existing(
                hooked::ActiveModel {
                    id: Set(1),
                    ..Default::default()
                },
                [hooked::Column::Id, hooked::Column::Name],
            )
            .await
        })
        .await;

        assert!(
            matches!(&refused, Err(DbErr::AttrNotSet(column)) if column == "name"),
            "{refused:?}"
        );
        assert_eq!(hooked::SAVES.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// A hook the key leaves: refused before a statement, on the same pool.
    #[tokio::test]
    async fn an_on_conflict_insert_whose_hook_unsets_its_key_sends_nothing() {
        let refused = with_job_executor(pool(), async {
            Repo::<unsetting::Entity>::insert_unscoped_or_existing(
                unsetting::ActiveModel {
                    id: Set(1),
                    name: Set("ada".to_owned()),
                },
                [unsetting::Column::Name],
            )
            .await
        })
        .await;

        assert!(
            matches!(&refused, Err(DbErr::AttrNotSet(column)) if column == "name"),
            "{refused:?}"
        );
    }

    mod unsetting {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "unsetting")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        #[async_trait::async_trait]
        impl ActiveModelBehavior for ActiveModel {
            async fn before_save<C: ConnectionTrait>(
                mut self,
                _: &C,
                _: bool,
            ) -> Result<Self, DbErr> {
                self.name = sea_orm::ActiveValue::NotSet;
                Ok(self)
            }
        }
    }

    mod hooked {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use sea_orm::entity::prelude::*;

        /// How many saves reached the entity's `before_save`.
        pub(super) static SAVES: AtomicUsize = AtomicUsize::new(0);

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "hooked")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
            pub deleted_at: Option<DateTimeWithTimeZone>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        #[async_trait::async_trait]
        impl ActiveModelBehavior for ActiveModel {
            async fn before_save<C: ConnectionTrait>(self, _: &C, _: bool) -> Result<Self, DbErr> {
                SAVES.fetch_add(1, Ordering::SeqCst);
                Ok(self)
            }
        }
    }

    /// A save the ability bars on the whole class — a handler gated on
    /// another subject, so no class gate saw it — answers what a row outside
    /// the scope answers, before any hook runs or any statement is sent: the
    /// pool here dials nothing, so a statement sent would fail otherwise.
    #[tokio::test]
    async fn a_save_the_ability_bars_on_the_class_runs_no_hook_and_sends_nothing() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Manage, hooked::Entity)
            .when(|p| p.eq(hooked::Column::Id, 1));
        b.cannot(Action::Update, hooked::Entity);
        b.cannot(Action::Delete, hooked::Entity);
        let ability = Arc::new(b.build().expect("valid test ability"));
        let row = hooked::Model {
            id: 1,
            name: "ada".to_owned(),
            deleted_at: None,
        };

        let (updated, soft_deleted) = with_request_executor(
            pool(),
            install_ability(ability, async {
                let mut renamed = row.clone().into_active_model();
                renamed.name = Set("grace".to_owned());
                (
                    Repo::<hooked::Entity>::update(renamed).await,
                    Repo::<hooked::Entity>::soft_delete(row.clone(), hooked::Column::DeletedAt)
                        .await,
                )
            }),
        )
        .await;

        assert!(
            matches!(updated, Err(DbErr::RecordNotUpdated)),
            "{updated:?}"
        );
        assert!(
            matches!(soft_deleted, Err(DbErr::RecordNotUpdated)),
            "{soft_deleted:?}"
        );
        assert_eq!(
            hooked::SAVES.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no hook runs for a caller the ability bars",
        );
    }

    /// A request no ability reaches — a route whose posture names no subject,
    /// whose ability `UnmaskedReply` withholds — is barred like one its ability
    /// bars, saying why once; the pool dials nothing, so a statement sent would fail otherwise.
    #[tokio::test]
    async fn a_save_in_a_request_holding_no_ability_runs_no_hook_and_sends_nothing() {
        let logs = nest_rs_testing::LogCapture::install();
        let row = hooked::Model {
            id: 1,
            name: "ada".to_owned(),
            deleted_at: None,
        };

        let (updated, soft_deleted) = with_request_executor(pool(), async {
            let mut renamed = row.clone().into_active_model();
            renamed.name = Set("grace".to_owned());
            (
                Repo::<hooked::Entity>::update(renamed).await,
                Repo::<hooked::Entity>::soft_delete(row.clone(), hooked::Column::DeletedAt).await,
            )
        })
        .await;

        assert!(
            matches!(updated, Err(DbErr::RecordNotUpdated)),
            "{updated:?}"
        );
        assert!(
            matches!(soft_deleted, Err(DbErr::RecordNotUpdated)),
            "{soft_deleted:?}"
        );
        assert_eq!(hooked::SAVES.load(std::sync::atomic::Ordering::SeqCst), 0);
        let said = logs.find(
            crate::target::ORM,
            "no ambient Ability outside a worker job — denying all rows",
        );
        assert_eq!(said.len(), 2, "{said:#?}");
        assert!(said.iter().all(|event| event.level == "warn"), "{said:#?}");
        assert!(
            said.iter().all(|event| event
                .field("hint")
                .is_some_and(|hint| hint.contains("connection hook"))),
            "a hook declares no posture, so the line says where to read instead: {said:#?}",
        );
    }

    /// A job holds no ability and is unscoped: its save reaches the hook and the
    /// statement, which meets a pool whose every acquire is refused.
    #[tokio::test]
    async fn a_save_in_a_job_runs_its_hook_and_sends_its_statement() {
        let mut renamed = hooked::Model {
            id: 1,
            name: "ada".to_owned(),
            deleted_at: None,
        }
        .into_active_model();
        renamed.name = Set("grace".to_owned());

        let updated = with_job_executor(
            a_pool_refusing_every_statement().await,
            Repo::<hooked::Entity>::update(renamed),
        )
        .await;

        assert!(
            !matches!(updated, Ok(_) | Err(DbErr::RecordNotUpdated)),
            "{updated:?}"
        );
        assert_eq!(hooked::SAVES.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A visitor's ability is held and withheld where no line names its read, a
    /// nested field's or a relation's batch's, and that field has no posture to
    /// write: the line says so rather than "no ambient Ability".
    #[tokio::test]
    async fn a_visitors_read_off_the_unit_naming_it_says_what_withheld_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let visitor = AbilityBuilder::new().build_visitor().expect("no rule");

        let barred = nest_rs_core::with_ambient(
            std::sync::Arc::new(nest_rs_authz::__private::AmbientAbility::new(visitor)),
            async { barred_from_class::<widget::Entity>(Action::Read) },
        )
        .await;

        assert!(barred, "a withheld read still fails closed");
        logs.expect_none(
            crate::target::ORM,
            "no ambient Ability outside a worker job — denying all rows",
        );
        let event = logs.expect_one(
            crate::target::ORM,
            "a visitor's read outside the unit naming it — denying all rows",
        );
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("hint")
                .is_some_and(|hint| hint.contains("nested GraphQL field")),
            "{:?}",
            event.fields,
        );
    }

    /// A grant's `.fields` names what its caller may write: a column it leaves out
    /// is refused by name before a hook runs or a statement is sent, on a pool that
    /// dials nothing, and the operator's line says which, never a value.
    #[tokio::test]
    async fn a_write_of_a_column_its_grant_leaves_out_is_refused_with_no_statement() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, hooked::Entity)
            .fields([hooked::Column::Name]);
        let ability = Arc::new(b.build().expect("valid test ability"));
        let mut written = hooked::Model {
            id: 1,
            name: "ada".to_owned(),
            deleted_at: None,
        }
        .into_active_model();
        written.name = Set("secret-name".to_owned());
        written.deleted_at = Set(Some(crate::now()));

        let refused = with_request_executor(
            pool(),
            install_ability(ability, Repo::<hooked::Entity>::update(written)),
        )
        .await;

        assert!(
            matches!(
                &refused,
                Err(DbErr::AccessDenied { permission, resource })
                    if permission == "update" && resource == "hooked(deleted_at)"
            ),
            "{refused:?}",
        );
        assert_eq!(hooked::SAVES.load(std::sync::atomic::Ordering::SeqCst), 0);
        let event = logs.expect_one(
            crate::target::ORM,
            "access denied — a column the caller's grants withhold",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("entity").as_deref(), Some("hooked"));
        assert_eq!(event.field("action").as_deref(), Some("update"));
        assert_eq!(event.field("columns").as_deref(), Some("deleted_at"));
        assert!(!event.quotes("secret-name"), "{event:#?}");
    }

    /// A lazy pool whose every acquire is refused: a write answering its error
    /// sent its statement, where one refused earlier answers that refusal.
    async fn a_pool_refusing_every_statement() -> Arc<dyn Executor> {
        let mut options = sea_orm::ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
        options
            .connect_lazy(true)
            .acquire_timeout(std::time::Duration::from_millis(50));
        let unreachable = sea_orm::Database::connect(options)
            .await
            .expect("a lazy pool opens nothing");
        SeaOrmExecutor::Pool(unreachable).into()
    }

    mod stamped {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use sea_orm::entity::prelude::*;

        /// How many saves the entity's `before_save` stamped.
        pub(super) static STAMPS: AtomicUsize = AtomicUsize::new(0);

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "stamped")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
            pub note: Option<String>,
            pub updated_at: Option<DateTimeWithTimeZone>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        #[async_trait::async_trait]
        impl ActiveModelBehavior for ActiveModel {
            async fn before_save<C: ConnectionTrait>(
                mut self,
                _: &C,
                _: bool,
            ) -> Result<Self, DbErr> {
                STAMPS.fetch_add(1, Ordering::SeqCst);
                self.updated_at = sea_orm::Set(Some(crate::now()));
                Ok(self)
            }
        }
    }

    fn a_stamped_row() -> stamped::Model {
        stamped::Model {
            id: 1,
            name: "ada".to_owned(),
            note: None,
            updated_at: None,
        }
    }

    /// What the entity's hook stamps is the server's: no grant names it, and the
    /// write still reaches its statement.
    #[tokio::test]
    async fn a_hooks_stamp_needs_no_grant() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, stamped::Entity)
            .fields([stamped::Column::Name]);
        let ability = Arc::new(b.build().expect("valid test ability"));
        let mut renamed = a_stamped_row().into_active_model();
        renamed.name = Set("grace".to_owned());

        let updated = with_request_executor(
            a_pool_refusing_every_statement().await,
            install_ability(ability, Repo::<stamped::Entity>::update(renamed)),
        )
        .await;

        assert!(
            !matches!(
                updated,
                Ok(_) | Err(DbErr::RecordNotUpdated | DbErr::AccessDenied { .. })
            ),
            "{updated:?}"
        );
        assert_eq!(stamped::STAMPS.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A soft delete's stamp is the framework's: a `.fields` on the wildcard covering
    /// `Delete` limits what a write chooses, and a delete chooses nothing.
    #[tokio::test]
    async fn a_soft_delete_stamps_whatever_a_field_grant_says() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Manage, hooked::Entity)
            .fields([hooked::Column::Name]);
        let ability = Arc::new(b.build().expect("valid test ability"));
        let row = hooked::Model {
            id: 1,
            name: "ada".to_owned(),
            deleted_at: None,
        };

        let deleted = with_request_executor(
            a_pool_refusing_every_statement().await,
            install_ability(
                ability,
                Repo::<hooked::Entity>::soft_delete(row, hooked::Column::DeletedAt),
            ),
        )
        .await;

        assert!(
            !matches!(
                deleted,
                Ok(()) | Err(DbErr::RecordNotUpdated | DbErr::AccessDenied { .. })
            ),
            "{deleted:?}"
        );
    }

    /// A job holds no ability and is withheld nothing; `with_ability` is what
    /// narrows one, to a principal's columns too.
    #[tokio::test]
    async fn a_job_is_withheld_no_column_until_it_narrows_to_a_principals() {
        let mut written = a_stamped_row().into_active_model();
        written.note = Set(Some("mine".to_owned()));

        let system = with_job_executor(pool(), async { write_scope(Action::Update, &written) })
            .await
            .expect("system work");
        assert!(system.is_empty(), "unscoped");

        let mut b = AbilityBuilder::new();
        b.can(Action::Update, stamped::Entity)
            .fields([stamped::Column::Name]);
        let principal = Arc::new(b.build().expect("valid test ability"));
        let narrowed = with_job_executor(
            pool(),
            nest_rs_authz::with_ability(principal, async { write_scope(Action::Update, &written) }),
        )
        .await;
        assert!(
            matches!(&narrowed, Err(DbErr::AccessDenied { resource, .. }) if resource == "stamped(note)"),
            "{narrowed:?}",
        );
    }

    /// A column holding `sea_query::Enum` itself, which sea-orm reads and writes as
    /// it is: what names its type is the value, not a derive.
    #[expect(
        clippy::disallowed_types,
        reason = "the column under test: its type name is the value's"
    )]
    mod labelled {
        use sea_orm::entity::prelude::*;
        use sea_orm::sea_query::Enum;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "labelled")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            #[sea_orm(column_type = "Text")]
            pub label: Enum,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}

        /// A value whose type name holds a `"`, which ends the quotes sea-query writes
        /// it between, and a comment that reads the rest of the statement as text.
        pub(super) fn escaping() -> Enum {
            Enum {
                type_name: r#"text" OR true --"#.into(),
                value: "a".into(),
            }
        }
    }

    #[expect(clippy::disallowed_types, reason = "the column under test")]
    fn labelled_row(label: sea_orm::sea_query::Enum) -> labelled::ActiveModel {
        labelled::ActiveModel {
            id: Set(1),
            label: Set(label),
        }
    }

    #[test]
    #[expect(clippy::disallowed_methods, reason = "a scope denying every row")]
    fn an_enum_type_name_leaving_its_quotes_would_drop_a_writes_scope() {
        let deny = || Condition::all().add(Expr::cust("1 = 0"));
        let update = Update::one(labelled_row(labelled::escaping()))
            .validate()
            .expect("a PK-bearing active model validates")
            .filter(deny())
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            update.contains(r#"SET "label" = 'a'::"text" OR true --" WHERE"#),
            "every row is written: {update}",
        );
    }

    /// Each write sends nothing for such a value: an update or a soft delete, its hook
    /// run first, and an insert. A key cannot hold one: sea-orm reads no key into it.
    #[tokio::test]
    async fn a_write_carrying_an_enum_type_name_leaving_its_quotes_sends_nothing() {
        let refused = |answer: &DbErr| answer.to_string().contains("refused before any statement");
        type Labelled = Repo<labelled::Entity>;
        let answers = with_job_executor(pool(), async {
            let carrying = || labelled_row(labelled::escaping());
            [
                Labelled::update(carrying()).await.map(drop),
                Labelled::insert_unscoped(carrying()).await.map(drop),
                Labelled::insert_unscoped_or_existing(carrying(), [labelled::Column::Id])
                    .await
                    .map(drop),
            ]
        })
        .await;
        for answer in answers {
            assert!(answer.as_ref().is_err_and(refused), "{answer:?}");
        }
    }

    /// The `WHERE` of `written`'s statement, as `Repo::update` sends it.
    fn update_where<A>(written: A, scope: Condition) -> String
    where
        A: ActiveModelTrait,
    {
        let sql = Update::one(written)
            .validate()
            .expect("a PK-bearing active model validates")
            .filter(scope)
            .build(DatabaseBackend::Postgres)
            .to_string();
        sql.split_once(" WHERE ")
            .map_or(String::new(), |(_, scope)| scope.to_owned())
    }

    #[tokio::test]
    async fn a_narrowed_write_runs_under_the_grants_permitting_what_it_sets() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, stamped::Entity)
            .when(|p| p.eq(stamped::Column::Id, 7))
            .fields([stamped::Column::Name]);
        b.can(Action::Update, stamped::Entity)
            .when(|p| p.eq(stamped::Column::Id, 8))
            .fields([stamped::Column::Note]);
        let ability = Arc::new(b.build().expect("valid test ability"));
        let mut written = a_stamped_row().into_active_model();
        written.name = Set("grace".to_owned());
        written.note = Set(None);

        let scope = with_request_executor(
            pool(),
            install_ability(ability, async { write_scope(Action::Update, &written) }),
        )
        .await
        .expect("a grant permits each column");

        assert_eq!(
            update_where(written, scope),
            r#""stamped"."id" = 1 AND "stamped"."id" = 7 AND "stamped"."id" = 8"#,
            "the key, then a grant per column, one statement deciding them all",
        );
    }

    /// A caller no `.fields` narrows sends the statement it always did.
    #[tokio::test]
    async fn a_write_no_grant_narrows_is_held_to_scope_for_alone() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, tombstone::Entity)
            .when(|p| p.eq(tombstone::Column::Id, 7));
        let ability = Arc::new(b.build().expect("valid test ability"));
        let written = tombstone::ActiveModel {
            id: Set(1),
            deleted_at: Set(None),
        };

        let (scope, plain) = with_request_executor(
            pool(),
            install_ability(ability, async {
                (
                    write_scope(Action::Update, &written).expect("nothing narrowed"),
                    scope_for::<tombstone::Entity>(Action::Update),
                )
            }),
        )
        .await;

        assert_eq!(
            update_where(written.clone(), scope),
            update_where(written, plain),
        );
    }

    /// Hiding a tombstone is no grant's to lift: a narrowed write keeps it.
    #[tokio::test]
    async fn a_narrowed_write_still_hides_a_tombstone() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, tombstone::Entity)
            .when(|p| p.eq(tombstone::Column::Id, 7))
            .fields([tombstone::Column::DeletedAt]);
        let ability = Arc::new(b.build().expect("valid test ability"));
        let written = tombstone::ActiveModel {
            id: Set(1),
            deleted_at: Set(None),
        };

        let scope = with_request_executor(
            pool(),
            install_ability(ability, async { write_scope(Action::Update, &written) }),
        )
        .await
        .expect("the grant permits it");

        let s = update_where(written, scope);
        assert!(
            s.contains(LIVE) && s.contains(r#""tombstones"."id" = 7"#),
            "{s}"
        );
    }

    #[test]
    fn a_write_chooses_what_it_sets_but_the_key() {
        let written = stamped::ActiveModel {
            id: Set(1),
            name: Set("grace".to_owned()),
            note: Set(None),
            updated_at: sea_orm::ActiveValue::NotSet,
        };

        assert_eq!(
            names(chosen_columns(Action::Update, &written)),
            ["name", "note"],
            "an update's null clears a column; the key addresses the row",
        );
        assert_eq!(
            names(chosen_columns(Action::Create, &written)),
            ["name"],
            "a create's null is a member its input did not carry; the key is seeded",
        );

        let loaded = a_stamped_row().into_active_model();
        assert!(
            chosen_columns(Action::Update, &loaded).is_empty(),
            "what a row loaded with is no choice",
        );
    }

    fn names<C: IdenStatic>(columns: Vec<C>) -> Vec<&'static str> {
        columns.iter().map(IdenStatic::as_str).collect()
    }

    #[tokio::test]
    async fn update_by_id_ands_the_pk_with_the_ability_scope() {
        let pool = pool();
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 91));
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            install_ability(ability, async {
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
        let pool = pool();
        let mut b = AbilityBuilder::new();
        b.can(Action::Delete, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 91));
        let ability = Arc::new(b.build().expect("valid test ability"));

        with_request_executor(pool, async move {
            install_ability(ability, async {
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
