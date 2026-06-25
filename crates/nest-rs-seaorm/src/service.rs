//! [`CrudService`] — the entity's data API and the single audited gateway to the
//! ORM. Controllers and resolvers delegate here; they never touch [`Repo`] or
//! the ORM directly, so there is exactly one choke point per entity to secure
//! and audit. Default methods express CRUD through [`Repo`], keeping ambient
//! scoping and the request transaction transparent.

use async_trait::async_trait;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Condition;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DbErr, EntityName, EntityTrait, IntoActiveModel,
    PrimaryKeyTrait, QueryFilter,
};

use nest_rs_authz::{Action, current_ability};

use crate::page::Page;
use crate::repo::Repo;

/// Build a fresh `ActiveModel` from a create-input DTO. Implemented by `#[expose]`
/// for each `Create<Name>Input`.
pub trait CreateModel<E: EntityTrait> {
    fn into_active_model(self) -> E::ActiveModel;
}

/// Apply an update-input DTO onto a loaded `ActiveModel`. Implemented by
/// `#[expose]` for each `Update<Name>Input`.
pub trait UpdateModel<E: EntityTrait> {
    fn apply_to(self, model: E::ActiveModel) -> E::ActiveModel;
}

/// Outcome of an authorized by-id load. Distinguishing `Denied` from `Missing`
/// lets a surface map to 200/403/404 (REST) or data/forbidden/null (GraphQL)
/// without leaking existence by silently returning `Missing` for a denied row.
pub enum Access<M> {
    Found(M),
    Denied,
    Missing,
}

/// Proof that the wrapped row was produced by an **authorized** load — the
/// ambient ability granted the binding action on it through
/// [`CrudService::access`], the single gateway every `Bind` /
/// [`bind_required`](crate::graphql::bind_required) funnels through.
///
/// A service method that accepts `Authorized<E>` is *statically* guaranteed its
/// subject passed that check: the type's constructor is crate-private, so it
/// cannot be minted from a raw `Entity::find_by_id` — a hand-written mutation
/// can no longer act on a row the caller was never allowed to load. The model is
/// read through [`Deref`](std::ops::Deref); [`into_inner`](Authorized::into_inner)
/// takes ownership for the active-model write (which [`Repo`](crate::Repo)
/// re-scopes by the ambient ability — defense in depth, not the only line).
pub struct Authorized<E: EntityTrait>(E::Model);

impl<E: EntityTrait> Authorized<E> {
    /// Mint the proof. **Crate-private on purpose**: only the binding seams that
    /// pass through [`CrudService::access`] may construct it, which is what makes
    /// the type a guarantee rather than a label.
    pub(crate) fn new(model: E::Model) -> Self {
        Self(model)
    }

    /// Take ownership of the authorized model — e.g. for `into_active_model`.
    pub fn into_inner(self) -> E::Model {
        self.0
    }
}

impl<E: EntityTrait> std::ops::Deref for Authorized<E> {
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
    async fn access(
        &self,
        action: Action,
        id: Uuid,
    ) -> Result<Access<<Self::Entity as EntityTrait>::Model>, DbErr>
    where
        <Self::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    {
        let conn = Repo::<Self::Entity>::conn()?;
        let query = Repo::<Self::Entity>::unscoped_by_id(id).filter(Self::live_read_filter());
        let Some(model) = query.one(&conn).await? else {
            return Ok(Access::Missing);
        };
        let allowed = current_ability()
            .map(|ability| ability.can::<Self::Entity>(action, &model))
            .unwrap_or(false);
        let entity = Self::entity_name();
        if allowed {
            tracing::debug!(target: "nest_rs::orm", entity, %id, ?action, "access granted");
            Ok(Access::Found(model))
        } else {
            tracing::warn!(target: "nest_rs::orm", entity, %id, ?action, "access denied");
            Ok(Access::Denied)
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
    type Create: CreateModel<Self::Entity> + Send;

    /// Insert a row from a create-input DTO, in the request transaction.
    ///
    /// Defense in depth beyond the route's `Authorize<Create, _>` gate: when an
    /// [`Ability`](nest_rs_authz::Ability) is ambient (any authenticated request
    /// path), the freshly built row is checked against `condition_for(Create)`
    /// and a row outside the caller's scope is rolled back via
    /// [`DbErr::RecordNotInserted`] — a caller cannot create a row it could not
    /// then read or update. With no ambient ability (system/worker path) the
    /// insert stays unscoped, mirroring the read default on a pool executor.
    async fn create(
        &self,
        input: Self::Create,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let entity = Self::entity_name();
        let conn = Repo::<Self::Entity>::conn()?;
        let model = input.into_active_model().insert(&conn).await?;
        if let Some(ability) = current_ability()
            && !ability.can::<Self::Entity>(Action::Create, &model)
        {
            tracing::warn!(
                target: "nest_rs::orm",
                entity,
                id = ?model_pk::<Self::Entity>(&model),
                action = ?Action::Create,
                "create denied — row outside the caller's scope",
            );
            return Err(DbErr::RecordNotInserted);
        }
        tracing::debug!(target: "nest_rs::orm", entity, id = ?model_pk::<Self::Entity>(&model), "row created");
        Ok(model)
    }
}

/// Opt-in write capability: the resource accepts **updates**. Carries the
/// update-input type and the audited `update` path. Implemented only when the
/// resource genuinely mutates rows — so a `update<E>` op is never generated for
/// a resource that has no honest update to apply.
#[async_trait]
pub trait Updatable: CrudService {
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
                    "update denied — row outside the caller's scope",
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
                        "soft-delete denied — row outside the caller's scope",
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
                        "delete denied — row outside the caller's scope",
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
/// denial/mutation events. Generic over the entity; every entity has at least
/// one primary-key column (mirrors the `page` cursor's `expect`).
fn model_pk<E: EntityTrait>(model: &E::Model) -> sea_orm::Value {
    use sea_orm::{Iterable, ModelTrait, PrimaryKeyToColumn};
    let pk_col = E::PrimaryKey::iter()
        .next()
        .expect("an entity has at least one primary-key column")
        .into_column();
    model.get(pk_col)
}
