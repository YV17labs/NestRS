//! [`CrudService`] — the entity's data API and the single audited gateway to the
//! ORM.
//!
//! A service is *the* place an entity's rows are read and written. Controllers
//! and resolvers never touch [`Repo`] or the ORM directly — they delegate here,
//! so there is exactly one choke point per entity to secure and audit (the
//! by-id route-model binding loads through [`access`](CrudService::access) too).
//!
//! "Inheriting a base CRUD" is a trait with default methods: a service names
//! three associated types and gets [`list`](CrudService::list)/
//! [`page`](CrudService::page)/[`access`](CrudService::access)/
//! [`create`](CrudService::create)/[`update`](CrudService::update)/
//! [`delete`](CrudService::delete) for free, all expressed through [`Repo`] (so
//! the ambient scoping and request transaction stay transparent). Override any
//! method to add business logic — e.g. stamp a tenant column on `create`.

use async_trait::async_trait;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DbErr, EntityName, EntityTrait, IntoActiveModel,
    PrimaryKeyTrait,
};

use nestrs_authz::{current_ability, Action};

use crate::page::Page;
use crate::repo::Repo;

/// Build a fresh `ActiveModel` from a create-input DTO. Implemented by `#[expose]`
/// for each `Create<Name>Input`, so [`CrudService::create`] is generic.
pub trait CreateModel<E: EntityTrait> {
    fn into_active_model(self) -> E::ActiveModel;
}

/// Apply an update-input DTO onto a loaded `ActiveModel`. Implemented by
/// `#[expose]` for each `Update<Name>Input`, so [`CrudService::update`] is generic.
pub trait UpdateModel<E: EntityTrait> {
    fn apply_to(self, model: E::ActiveModel) -> E::ActiveModel;
}

/// The outcome of an authorized by-id load: the row, a denial (it exists but the
/// caller may not act on it), or absence — so a surface maps them to 200/403/404
/// (REST) or data/forbidden/null (GraphQL) without hiding existence.
pub enum Access<M> {
    Found(M),
    Denied,
    Missing,
}

/// The entity's CRUD API. Implement it with the three associated types to inherit
/// every method; override any to extend it. The `where` clause names the SeaORM
/// bounds a derived entity already satisfies (its `ActiveModel` has behaviour, its
/// `Model` converts to an `ActiveModel`), so the default bodies can insert/update/
/// delete generically.
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

    /// The entity's table name, the `entity` field every operation log carries
    /// (the flat module path can't distinguish entities — they all log from
    /// `nestrs_database::service`).
    fn entity_name() -> &'static str {
        Self::Entity::default().table_name()
    }

    /// Every row the caller may [`Read`](Action::Read), ability-scoped by `Repo`.
    async fn list(&self) -> Result<Vec<<Self::Entity as EntityTrait>::Model>, DbErr> {
        tracing::debug!(target: "nestrs::orm", entity = Self::entity_name(), "listing rows");
        Repo::<Self::Entity>::all().await
    }

    /// A keyset page of readable rows, ascending by primary key (see [`Repo::page`]).
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
    /// `Bind`/`bind` adapters delegate to. The caller's ability is read from the
    /// ambient request state the route gate installed.
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

    /// Apply an update-input DTO to a loaded row, in the request transaction. The
    /// write is ability-scoped by [`Repo::update`]: a row outside the caller's
    /// scope is never touched and surfaces as [`DbErr::RecordNotUpdated`] (logged
    /// at `warn`), so a caller cannot mutate by id past its scope even if it
    /// reached this method with a row loaded some other way.
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

    /// Delete a loaded row, in the request transaction. The write is ability-scoped
    /// by [`Repo::delete`]: a row outside the caller's scope is not deleted, which
    /// surfaces as a zero-row result mapped to [`DbErr::RecordNotFound`] (logged at
    /// `warn`) so a caller cannot delete by id past its scope.
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
