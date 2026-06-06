//! [`CrudService`] — the entity's data API and the single audited gateway to the
//! ORM. Controllers and resolvers delegate here; they never touch [`Repo`] or
//! the ORM directly, so there is exactly one choke point per entity to secure
//! and audit. Default methods express CRUD through [`Repo`], keeping ambient
//! scoping and the request transaction transparent.

use async_trait::async_trait;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DbErr, EntityName, EntityTrait, IntoActiveModel,
    PrimaryKeyTrait,
};

use nestrs_authz::{Action, current_ability};

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
    /// `nestrs_seaorm::service`).
    fn entity_name() -> &'static str {
        Self::Entity::default().table_name()
    }

    /// Every row the caller may [`Read`](Action::Read), ability-scoped by `Repo`.
    async fn list(&self) -> Result<Vec<<Self::Entity as EntityTrait>::Model>, DbErr> {
        tracing::debug!(target: "nestrs::orm", entity = Self::entity_name(), "listing rows");
        Repo::<Self::Entity>::all().await
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
        tracing::debug!(target: "nestrs::orm", entity = Self::entity_name(), first, ?after, "paging rows");
        Repo::<Self::Entity>::page(first, after).await
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
        let Some(model) = Self::Entity::find_by_id(id).one(&conn).await? else {
            return Ok(Access::Missing);
        };
        let allowed = current_ability()
            .map(|ability| ability.can::<Self::Entity>(action, &model))
            .unwrap_or(false);
        let entity = Self::entity_name();
        if allowed {
            tracing::debug!(target: "nestrs::orm", entity, %id, ?action, "access granted");
            Ok(Access::Found(model))
        } else {
            tracing::warn!(target: "nestrs::orm", entity, %id, ?action, "access denied");
            Ok(Access::Denied)
        }
    }

    /// Insert a row from a create-input DTO, in the request transaction.
    async fn create(
        &self,
        input: Self::Create,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let conn = Repo::<Self::Entity>::conn()?;
        let model = input.into_active_model().insert(&conn).await?;
        tracing::info!(target: "nestrs::orm", entity = Self::entity_name(), "row created");
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
        let active = input.apply_to(model.into_active_model());
        match Repo::<Self::Entity>::update(active).await {
            Ok(updated) => {
                tracing::info!(target: "nestrs::orm", entity, "row updated");
                Ok(updated)
            }
            Err(DbErr::RecordNotUpdated) => {
                tracing::warn!(target: "nestrs::orm", entity, "update denied — row outside the caller's scope");
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
        let result = Repo::<Self::Entity>::delete(model).await?;
        if result.rows_affected == 0 {
            tracing::warn!(target: "nestrs::orm", entity, "delete denied — row outside the caller's scope");
            return Err(DbErr::RecordNotFound(format!(
                "{entity} row not found or outside the caller's scope"
            )));
        }
        tracing::info!(target: "nestrs::orm", entity, "row deleted");
        Ok(())
    }
}
