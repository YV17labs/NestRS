//! [`Repo`] — the query entry point that makes security and transactions
//! transparent. Every call runs against the ambient
//! [`Executor`](crate::Executor) (the request's transaction when open) and is
//! filtered by the caller's [`Ability`](nestrs_authz::Ability): reads via
//! `condition_for(Read)`, by-id writes via `condition_for(Update/Delete)` ANDed
//! with the primary key — so a caller cannot mutate a row outside its scope
//! even by id. Worker/system paths run unscoped (no ability ⇒ TRUE); a
//! request-scoped executor without an ability denies every row (fail-closed).
//! A call outside the executor scope errors rather than silently reaching a
//! connection it does not have.

use std::marker::PhantomData;

use nestrs_authz::{Action, current_ability};
use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{
    ActiveModelTrait, DbErr, Delete, DeleteResult, EntityTrait, IntoActiveModel, PrimaryKeyTrait,
    QueryFilter, Select, Update,
};

use crate::executor::{Executor, ExecutorScope, current_executor, current_executor_scope};

/// Row-level filter for `action` on `E` from the ambient ability. Deny-all on a
/// request-scoped executor without an ability; unscoped on worker/system paths.
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
pub struct Repo<E: EntityTrait>(PhantomData<fn() -> E>);

impl<E: EntityTrait> Repo<E> {
    /// The ambient executor (transaction when open, else the pool), for a write
    /// or a custom query: `active.insert(&Repo::<E>::conn()?)`.
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
}
