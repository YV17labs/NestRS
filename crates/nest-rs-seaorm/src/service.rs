//! [`CrudService`] — the entity's data API and the single audited gateway to the
//! ORM. Controllers and resolvers delegate here; they never touch [`Repo`] or
//! the ORM directly, so there is exactly one choke point per entity to secure
//! and audit. Default methods express CRUD through [`Repo`], keeping ambient
//! scoping and the request transaction transparent.

use std::marker::PhantomData;

use async_trait::async_trait;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Condition;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ConnectionTrait, DbErr, EntityName, EntityTrait,
    IntoActiveModel, PrimaryKeyTrait, QueryFilter, TransactionTrait,
};

use nest_rs_authz::{Action, ActionMarker};

use crate::executor::Executor;
use crate::page::Page;
use crate::repo::{Repo, scope_for};

/// Build a fresh `ActiveModel` from a create-input DTO. Implemented by `#[expose]`
/// for each `Create<Name>Input`.
pub trait CreateModel<E: EntityTrait> {
    /// Turn the create-input DTO into a fresh `ActiveModel` ready to insert.
    fn into_active_model(self) -> E::ActiveModel;
}

/// Apply an update-input DTO onto a loaded `ActiveModel`. Implemented by
/// `#[expose]` for each `Update<Name>Input`.
pub trait UpdateModel<E: EntityTrait> {
    /// Apply the update-input DTO's set fields onto the loaded `ActiveModel`,
    /// leaving unset fields untouched.
    fn apply_to(self, model: E::ActiveModel) -> E::ActiveModel;
}

/// Outcome of an authorized by-id load. Distinguishing `Denied` from `Missing`
/// lets a surface map to 200/403/404 (REST) or data/forbidden/null (GraphQL)
/// without leaking existence by silently returning `Missing` for a denied row.
pub enum Access<M> {
    /// The row exists and the ability grants the action — carries the row.
    Found(M),
    /// The row exists but the ability denies the action (maps to 403).
    Denied,
    /// No such row (maps to 404). Kept distinct from `Denied` only where
    /// leaking existence is acceptable; the by-id write paths collapse both.
    Missing,
}

/// Proof that the wrapped row was produced by an **authorized** load — the
/// ambient ability granted action `A` on it through [`CrudService::access`], the
/// single gateway every `Bind` / `bind_required`
/// funnels through.
///
/// The action is carried in the type, not just the binding site: an
/// `Authorized<Update, E>` is a *different type* from an `Authorized<Read, E>`,
/// so a service method that takes `Authorized<Update, E>` is statically
/// guaranteed its subject was authorized for **exactly that action** — a `Read`
/// proof fed to a method expecting an `Update` proof is a type error, not a
/// runtime surprise. Combined with the crate-private constructor (only the
/// binding seams that pass through [`CrudService::access`] may mint one), the
/// type *is* the policy: a hand-written mutation can neither act on a row the
/// caller was never allowed to load, nor act under an action it was never
/// granted. The model is read through [`Deref`](std::ops::Deref);
/// [`into_inner`](Authorized::into_inner) takes ownership for the active-model
/// write (which [`Repo`](crate::Repo) re-scopes by the ambient ability — defense
/// in depth, not the only line).
pub struct Authorized<A: ActionMarker, E: EntityTrait>(E::Model, PhantomData<fn() -> A>);

impl<A: ActionMarker, E: EntityTrait> Authorized<A, E> {
    /// Mint the proof. **Crate-private on purpose**: only the binding seams that
    /// pass through [`CrudService::access`] may construct it, which is what makes
    /// the type a guarantee rather than a label. The only such seam is the
    /// `graphql` bridge, hence the gate: without it, nothing may mint a proof.
    #[cfg(feature = "graphql")]
    pub(crate) fn new(model: E::Model) -> Self {
        Self(model, PhantomData)
    }

    /// Take ownership of the authorized model — e.g. for `into_active_model`.
    pub fn into_inner(self) -> E::Model {
        self.0
    }
}

impl<A: ActionMarker, E: EntityTrait> std::ops::Deref for Authorized<A, E> {
    type Target = E::Model;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The entity's **read** API and the single audited gateway to the ORM. Every
/// resource implements it; the write half is segregated into the opt-in
/// [`Creatable`], [`Updatable`], and [`Deletable`] traits so a resource carries
/// — and exposes — only the operations it genuinely has. A read-only resource
/// (e.g. a relation or a projection) implements just this trait and never has
/// to declare an unused `Create`/`Update` placeholder.
#[async_trait]
pub trait CrudService: Send + Sync
where
    <Self::Entity as EntityTrait>::ActiveModel: ActiveModelBehavior + Send,
    <Self::Entity as EntityTrait>::Model:
        Send + Sync + IntoActiveModel<<Self::Entity as EntityTrait>::ActiveModel>,
{
    /// The SeaORM entity this service is the audited API for.
    type Entity: EntityTrait;

    /// The entity's table name; included as the `entity` field on every log
    /// (the flat module path can't distinguish entities — they all log from
    /// `nest_rs_seaorm::service`).
    fn entity_name() -> &'static str {
        Self::Entity::default().table_name()
    }

    /// Soft-delete opt-in. Override on the service to return the entity's
    /// `deleted_at` column. `None` (default) ⇒ hard delete and unfiltered reads
    /// — exactly today's behaviour.
    fn soft_delete_column() -> Option<<Self::Entity as EntityTrait>::Column> {
        None
    }

    /// The `WHERE` that hides soft-deleted rows: `deleted_at IS NULL` when
    /// [`soft_delete_column`](Self::soft_delete_column) is set, else `TRUE`.
    /// `Repo` reads AND this onto the ability scope.
    fn live_read_filter() -> Condition {
        match Self::soft_delete_column() {
            Some(col) => crate::soft_delete::live_condition_for_column(col),
            None => Condition::all(),
        }
    }

    /// Every row the caller may [`Read`](Action::Read), ability-scoped by
    /// `Repo` — up to [`LIST_CAP`](crate::LIST_CAP) rows. The cap is a
    /// backstop so no endpoint built on `list` can ever return an unbounded
    /// result set; a capped result logs a `warn`. Collections that may
    /// legitimately exceed it paginate with [`page`](CrudService::page).
    async fn list(&self) -> Result<Vec<<Self::Entity as EntityTrait>::Model>, DbErr> {
        use sea_orm::QuerySelect;
        tracing::debug!(target: "nest_rs::orm", entity = Self::entity_name(), "listing rows");
        let conn = Repo::<Self::Entity>::conn()?;
        let rows = Repo::<Self::Entity>::scoped(Action::Read)
            .filter(Self::live_read_filter())
            .limit(crate::LIST_CAP + 1)
            .all(&conn)
            .await?;
        let (rows, capped) = crate::page::split_overfetched(rows, crate::LIST_CAP);
        if capped {
            tracing::warn!(
                target: "nest_rs::orm",
                entity = Self::entity_name(),
                cap = crate::LIST_CAP,
                "list result truncated at the hard cap"
            );
        }
        Ok(rows)
    }

    /// A keyset page of readable rows, ascending by primary key.
    async fn page(
        &self,
        first: u64,
        after: Option<Uuid>,
    ) -> Result<Page<<Self::Entity as EntityTrait>::Model>, DbErr>
    where
        <Self::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
        <Self::Entity as EntityTrait>::Model: Send + Sync,
    {
        tracing::debug!(target: "nest_rs::orm", entity = Self::entity_name(), first, ?after, "paging rows");
        Repo::<Self::Entity>::page(first, after, Self::live_read_filter()).await
    }

    /// Load a row by id and authorize the caller for `action` on it. The load is
    /// **unscoped** so a denied-but-existing row is [`Access::Denied`] rather than
    /// hidden as [`Access::Missing`] — the route-model-binding gateway the
    /// `Bind`/`bind` adapters delegate to.
    ///
    /// Authorization is decided in **SQL**, by loading the id under
    /// `condition_for(action)` (the very `WHERE` the list path uses). This is
    /// one source of truth for every predicate kind: a relational rule —
    /// which an in-memory check cannot evaluate without loading the parent —
    /// is enforced here exactly as it is on a list read.
    ///
    /// The authorized path costs **one** primary-key lookup; the second,
    /// unscoped one runs only to separate `Denied` from `Missing` after the
    /// scoped load came back empty.
    async fn access(
        &self,
        action: Action,
        id: Uuid,
    ) -> Result<Access<<Self::Entity as EntityTrait>::Model>, DbErr>
    where
        <Self::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    {
        let conn = Repo::<Self::Entity>::conn()?;
        let live = Self::live_read_filter();
        let entity = Self::entity_name();
        // Ability-scoped first: an authorized load — the case every `Bind` and
        // bound mutation takes — then costs **one** round-trip. The unscoped
        // lookup below runs only when this one comes back empty, purely to tell
        // "exists but forbidden" from "absent"; the previous order paid for that
        // distinction on every successful access.
        if let Some(model) = Repo::<Self::Entity>::unscoped_by_id(id)
            .filter(scope_for::<Self::Entity>(action))
            .filter(live.clone())
            .one(&conn)
            .await?
        {
            tracing::debug!(target: "nest_rs::orm", entity, %id, ?action, "access granted");
            return Ok(Access::Found(model));
        }
        let exists = Repo::<Self::Entity>::unscoped_by_id(id)
            .filter(live)
            .one(&conn)
            .await?
            .is_some();
        if exists {
            tracing::warn!(target: "nest_rs::orm", entity, %id, ?action, "access denied");
            Ok(Access::Denied)
        } else {
            Ok(Access::Missing)
        }
    }
}

/// Opt-in write capability: the resource accepts **inserts**. Carries the
/// create-input type and the audited `create` path. A resource implements it
/// only when it genuinely creates rows — so there is no placeholder `Create`
/// type, and `#[crud(ops = [..create..])]` cannot generate a `create` op for a
/// resource that does not offer one.
#[async_trait]
pub trait Creatable: CrudService {
    /// The create-input DTO this resource accepts, lowered to an `ActiveModel`
    /// via [`CreateModel`].
    type Create: CreateModel<Self::Entity> + Send;

    /// Insert a row from a create-input DTO, atomically with its scope check.
    ///
    /// Defense in depth beyond the route's `Authorize<Create, _>` gate: the
    /// freshly inserted row is re-checked **in SQL** against
    /// `condition_for(Create)` — the same source of truth the read filter uses
    /// — and a row outside the caller's scope surfaces as
    /// [`DbErr::RecordNotInserted`] and never persists. Deciding in SQL
    /// (rather than an in-memory `can`) covers every predicate kind, including
    /// a **relational** Create grant whose scope lives on a parent row the
    /// in-memory check cannot reach — so a caller cannot create a child under
    /// an out-of-scope parent.
    ///
    /// Atomicity does not depend on the ambient executor's shape: on the
    /// request transaction (HTTP `DbContext`) insert + re-check ride it and
    /// the interceptor rolls back; on a **pool** executor (a WS message
    /// handler, a bare `with_executor`) a local transaction wraps the pair,
    /// committing only when the re-check passes. On a worker/system (`Job`)
    /// executor with no ambient ability `scope_for` is unscoped, so the
    /// insert stands, mirroring the read default there.
    async fn create(
        &self,
        input: Self::Create,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        self.create_from_active(input.into_active_model()).await
    }

    /// Insert a **prepared** `ActiveModel` through the same audited path as
    /// [`create`](Creatable::create) — atomic insert + SQL scope re-check —
    /// for service methods that stamp server-side columns (the token's org
    /// id, a status default) before insert. This is the sanctioned seam for
    /// those writes; a raw `ActiveModel::insert(&Repo::conn()?)` bypasses the
    /// ability pre-filter.
    async fn create_from_active(
        &self,
        active: <Self::Entity as EntityTrait>::ActiveModel,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let entity = Self::entity_name();
        // Insert + scope re-check run inside a **nested** transaction — a
        // SAVEPOINT on an active request transaction (`Txn`/`Lazy`), or a
        // top-level transaction on the pool. This makes the pair atomic
        // regardless of the ambient shape AND regardless of whether the handler
        // propagates the denial: a failed re-check rolls this local transaction
        // back, so a swallowed `RecordNotInserted` (`let _ = svc.create(..)`)
        // can never leave an out-of-scope row to be committed with the rest of
        // the request (DATA-S1). Committing the SAVEPOINT keeps the inserted row
        // in the outer request transaction, which still commits on 2xx.
        let local = match Repo::<Self::Entity>::conn()? {
            Executor::Pool(pool) => pool.begin().await?,
            Executor::Txn(txn) => txn.begin().await?,
            Executor::Lazy(lazy) => lazy.begin_nested().await?,
        };
        match insert_in_scope::<Self::Entity, _>(active, entity, &local).await {
            Ok(model) => {
                local.commit().await?;
                Ok(model)
            }
            Err(err) => {
                if let Err(rollback_err) = local.rollback().await {
                    tracing::error!(
                        target: "nest_rs::orm",
                        entity,
                        error = %rollback_err,
                        "rollback of the create SAVEPOINT/transaction failed",
                    );
                }
                Err(err)
            }
        }
    }
}

/// The audited create body shared by both `create_from_active` arms: insert,
/// then re-check the fresh row against `condition_for(Create)` **in SQL** on
/// the same connection. Generic over the connection so the ambient executor
/// and a local `DatabaseTransaction` use one implementation.
///
/// Deliberate asymmetry with `Repo`: reads and by-id writes pre-filter by
/// ability, but an insert has no existing row to filter, so there is no
/// `Repo::create` — the raw `insert` below is the one authorized-write entry
/// and the post-insert scope re-check (inside the caller's SAVEPOINT) is its
/// gate. `Repo::insert_unscoped` is the separate, principal-less escape.
async fn insert_in_scope<E, C>(
    active: E::ActiveModel,
    entity: &'static str,
    conn: &C,
) -> Result<E::Model, DbErr>
where
    E: EntityTrait,
    E::ActiveModel: ActiveModelBehavior + Send,
    E::Model: IntoActiveModel<E::ActiveModel>,
    C: ConnectionTrait,
{
    let model = active.insert(conn).await?;
    let in_scope = Repo::<E>::scoped(Action::Create)
        .filter(pk_condition::<E>(&model))
        .one(conn)
        .await?
        .is_some();
    if !in_scope {
        tracing::warn!(
            target: "nest_rs::orm",
            entity,
            id = ?model_pk::<E>(&model),
            action = ?Action::Create,
            "access denied — row outside the caller's scope",
        );
        return Err(DbErr::RecordNotInserted);
    }
    tracing::debug!(target: "nest_rs::orm", entity, id = ?model_pk::<E>(&model), "row created");
    Ok(model)
}

/// Opt-in write capability: the resource accepts **updates**. Carries the
/// update-input type and the audited `update` path. Implemented only when the
/// resource genuinely mutates rows — so a `update<E>` op is never generated for
/// a resource that has no honest update to apply.
#[async_trait]
pub trait Updatable: CrudService {
    /// The update-input DTO this resource accepts, applied to a loaded row via
    /// [`UpdateModel`].
    type Update: UpdateModel<Self::Entity> + Send;

    /// Apply an update-input DTO to a loaded row, in the request transaction.
    /// Ability-scoped by [`Repo::update`]: a row outside the caller's scope is
    /// never touched and surfaces as [`DbErr::RecordNotUpdated`], so a caller
    /// cannot mutate by id past its scope even if it reached this method with a
    /// row loaded some other way.
    async fn update(
        &self,
        model: <Self::Entity as EntityTrait>::Model,
        input: Self::Update,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let entity = Self::entity_name();
        let id = model_pk::<Self::Entity>(&model);
        let active = input.apply_to(model.into_active_model());
        match Repo::<Self::Entity>::update(active).await {
            Ok(updated) => {
                tracing::debug!(target: "nest_rs::orm", entity, ?id, "row updated");
                Ok(updated)
            }
            Err(DbErr::RecordNotUpdated) => {
                tracing::warn!(
                    target: "nest_rs::orm",
                    entity,
                    ?id,
                    action = ?Action::Update,
                    "access denied — row outside the caller's scope",
                );
                Err(DbErr::RecordNotUpdated)
            }
            Err(err) => Err(err),
        }
    }
}

/// Opt-in write capability: the resource accepts **deletes** (hard or, when
/// [`soft_delete_column`](CrudService::soft_delete_column) is set, soft). A
/// resource that is append-only simply does not implement it — and
/// `#[crud(ops = [..delete..])]` cannot expose a delete it does not have.
#[async_trait]
pub trait Deletable: CrudService {
    /// Delete a loaded row, in the request transaction. Ability-scoped by
    /// [`Repo::delete`]: a row outside the caller's scope yields a zero-row
    /// result mapped to [`DbErr::RecordNotFound`], so a caller cannot delete by
    /// id past its scope.
    async fn delete(&self, model: <Self::Entity as EntityTrait>::Model) -> Result<(), DbErr> {
        let entity = Self::entity_name();
        let id = model_pk::<Self::Entity>(&model);
        let out_of_scope = || {
            DbErr::RecordNotFound(format!(
                "{entity} row not found or outside the caller's scope"
            ))
        };
        match Self::soft_delete_column() {
            Some(col) => match Repo::<Self::Entity>::soft_delete(model, col).await {
                Ok(()) => {
                    tracing::debug!(target: "nest_rs::orm", entity, ?id, "row soft-deleted");
                    Ok(())
                }
                Err(DbErr::RecordNotUpdated) => {
                    tracing::warn!(
                        target: "nest_rs::orm",
                        entity,
                        ?id,
                        action = ?Action::Delete,
                        "access denied — row outside the caller's scope",
                    );
                    Err(out_of_scope())
                }
                Err(err) => Err(err),
            },
            None => {
                let result = Repo::<Self::Entity>::delete(model).await?;
                if result.rows_affected == 0 {
                    tracing::warn!(
                        target: "nest_rs::orm",
                        entity,
                        ?id,
                        action = ?Action::Delete,
                        "access denied — row outside the caller's scope",
                    );
                    return Err(out_of_scope());
                }
                tracing::debug!(target: "nest_rs::orm", entity, ?id, "row deleted");
                Ok(())
            }
        }
    }
}

/// First primary-key column value of a model, formatted for log correlation on
/// denial/mutation events, or `None` for a primary-key-less entity (SeaORM
/// permits them — views, raw tables). Logging-only, so a missing key logs as
/// `None` rather than panicking on this mutation hot path.
fn model_pk<E: EntityTrait>(model: &E::Model) -> Option<sea_orm::Value> {
    use sea_orm::{Iterable, ModelTrait, PrimaryKeyToColumn};
    let pk_col = E::PrimaryKey::iter().next()?.into_column();
    Some(model.get(pk_col))
}

/// Equality condition over **all** of a model's primary-key columns, used to
/// re-select a freshly inserted row for the scoped create re-check. Spans
/// composite keys, so it is correct for junction entities too.
fn pk_condition<E: EntityTrait>(model: &E::Model) -> Condition {
    use sea_orm::{ColumnTrait, Iterable, ModelTrait, PrimaryKeyToColumn};
    let mut cond = Condition::all();
    for pk in E::PrimaryKey::iter() {
        let col = pk.into_column();
        cond = cond.add(col.eq(model.get(col)));
    }
    cond
}
