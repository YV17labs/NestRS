//! [`CrudService`] — the entity's data API and the single audited gateway to the
//! ORM. Controllers and resolvers delegate here; they never touch [`Repo`] or
//! the ORM directly, so there is exactly one choke point per entity to secure
//! and audit. Default methods express CRUD through [`Repo`], keeping ambient
//! scoping and the request transaction transparent.

use async_trait::async_trait;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Condition;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DbErr, EntityName, EntityTrait,
    IntoActiveModel, PrimaryKeyTrait, QueryFilter,
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

/// The entity's CRUD API. Implement it with the three associated types to inherit
/// every method; override any to extend it.
#[async_trait]
pub trait CrudService: Send + Sync
where
    <Self::Entity as EntityTrait>::ActiveModel: ActiveModelBehavior + Send,
    <Self::Entity as EntityTrait>::Model:
        Send + Sync + IntoActiveModel<<Self::Entity as EntityTrait>::ActiveModel>,
{
    type Entity: EntityTrait;
    type Create: CreateModel<Self::Entity> + Send;
    type Update: UpdateModel<Self::Entity> + Send;

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

    /// Every row the caller may [`Read`](Action::Read), ability-scoped by `Repo`.
    async fn list(&self) -> Result<Vec<<Self::Entity as EntityTrait>::Model>, DbErr> {
        tracing::debug!(target: "nest_rs::orm", entity = Self::entity_name(), "listing rows");
        let conn = Repo::<Self::Entity>::conn()?;
        Repo::<Self::Entity>::scoped(Action::Read)
            .filter(Self::live_read_filter())
            .all(&conn)
            .await
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
        if let Some(ability) = current_ability() {
            if !ability.can::<Self::Entity>(Action::Create, &model) {
                tracing::warn!(
                    target: "nest_rs::orm",
                    entity,
                    id = ?model_pk::<Self::Entity>(&model),
                    action = ?Action::Create,
                    "create denied — row outside the caller's scope",
                );
                return Err(DbErr::RecordNotInserted);
            }
        }
        tracing::debug!(target: "nest_rs::orm", entity, id = ?model_pk::<Self::Entity>(&model), "row created");
        Ok(model)
    }

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

    /// Delete a loaded row, in the request transaction. Ability-scoped by
    /// [`Repo::delete`]: a row outside the caller's scope yields a zero-row
    /// result mapped to [`DbErr::RecordNotFound`], so a caller cannot delete by
    /// id past its scope.
    async fn delete(&self, model: <Self::Entity as EntityTrait>::Model) -> Result<(), DbErr> {
        let entity = Self::entity_name();
        let id = model_pk::<Self::Entity>(&model);
        let out_of_scope =
            || DbErr::RecordNotFound(format!("{entity} row not found or outside the caller's scope"));
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
