//! [`Repo`] — the query entry point that makes security and transactions
//! transparent.
//!
//! A service queries through `Repo::<E>` instead of holding a connection, so two
//! cross-cutting concerns disappear from its code:
//!
//! - **Transactions** — every `Repo` call runs against the *ambient*
//!   [`Executor`](crate::Executor), which is the request's transaction when one
//!   is open. The service never threads a transaction handle.
//! - **Row-level security** — every *read* is filtered by the caller's
//!   [`Ability`](nestrs_authz::Ability) (`condition_for`), read from the ambient
//!   request ability, and a [`update`](Repo::update)/[`delete`](Repo::delete) is
//!   gated the same way: its `WHERE` carries `condition_for(Update/Delete)` on top
//!   of the primary key, so a caller cannot mutate a row outside its scope even by
//!   id. A feature cannot forget to scope its reads or writes to what the caller
//!   may touch. Worker jobs and other non-request paths run unscoped when no
//!   ability is present; a **request-scoped** executor without an ability denies
//!   every row (fail-closed) and logs a warning.
//!
//! `Repo` requires an ambient executor (the [`DbContext`](crate::DbContext)
//! interceptor installs it per request); a call outside that scope errors rather
//! than silently reaching a connection it does not have. For a custom query, take
//! the ambient executor with [`Repo::conn`] and drive SeaORM directly.

use std::marker::PhantomData;

use nestrs_authz::{current_ability, Action};
use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{
    ActiveModelTrait, DbErr, Delete, DeleteResult, EntityTrait, IntoActiveModel, PrimaryKeyTrait,
    QueryFilter, Select, Update,
};

use crate::executor::{current_executor, current_executor_scope, Executor, ExecutorScope};

/// The caller's row-level filter for `action` on `E`, taken from the ambient
/// [`Ability`](nestrs_authz::Ability). Without an ability on a request-scoped
/// executor the filter is deny-all; on worker/system paths it is unscoped.
pub fn scope_for<E: EntityTrait>(action: Action) -> Condition {
    match current_ability() {
        Some(ability) => ability.condition_for::<E>(action),
        None if current_executor_scope() == Some(ExecutorScope::Request) => {
            tracing::warn!(
                target: "nestrs::orm",
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
/// Zero-sized — its methods are associated functions named at the call site
/// (`Repo::<users::Entity>::all()`).
pub struct Repo<E: EntityTrait>(PhantomData<fn() -> E>);

impl<E: EntityTrait> Repo<E> {
    /// The ambient request executor (the transaction when one is open, else the
    /// pool), for a write or a custom query: `active.insert(&Repo::<E>::conn()?)`.
    pub fn conn() -> Result<Executor, DbErr> {
        current_executor().ok_or_else(|| {
            DbErr::Custom(
                "no ambient database executor — a Repo query must run inside the request \
                 scope installed by nestrs-database's DbContext interceptor"
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

    /// A row by primary key, returned only if the caller may [`Read`](Action::Read)
    /// it — a row outside the caller's scope reads as `None`, never leaking its
    /// existence.
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
    /// query. Chain further constraints and execute against [`Repo::conn`], e.g.
    /// `Repo::<E>::scoped(Action::Update).all(&Repo::<E>::conn()?)`.
    pub fn scoped(action: Action) -> Select<E> {
        E::find().filter(scope_for::<E>(action))
    }

    /// Update a row from its `ActiveModel`, gated by the caller's ability: the
    /// write's `WHERE` carries [`condition_for(Update)`](nestrs_authz::Ability::condition_for)
    /// on top of the primary key, so a row outside the caller's scope is never
    /// touched. When the scope excludes the row the call errors
    /// [`DbErr::RecordNotUpdated`] — a caller cannot mutate by id past its scope.
    /// Runs against the ambient executor (the request transaction).
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

    /// Delete a row, gated by the caller's ability: the write's `WHERE` carries
    /// [`condition_for(Delete)`](nestrs_authz::Ability::condition_for) on top of
    /// the primary key, so a row outside the caller's scope is not deleted. The
    /// returned [`DeleteResult::rows_affected`] is `0` when the scope (or the row's
    /// absence) excluded it — the caller decides whether that is a denial. Runs
    /// against the ambient executor (the request transaction).
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
}
