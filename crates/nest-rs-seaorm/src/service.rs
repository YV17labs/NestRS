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

/// Outcome of an authorized by-id load: 200/403/404 on REST, data/forbidden/null
/// on GraphQL.
pub enum Access<M> {
    /// The row exists and the ability grants the action — carries the row.
    Found(M),
    /// The row exists but the ability denies the action (maps to 403).
    Denied,
    /// No such row (maps to 404). Kept distinct from `Denied` only where
    /// leaking existence is acceptable; the by-id write paths collapse both.
    Missing,
}

/// Proof that the ambient ability granted action `A` on the wrapped row through
/// [`CrudService::access`]; only the crate mints one. Read the model through
/// [`Deref`](std::ops::Deref), or own it with
/// [`into_inner`](Authorized::into_inner).
pub struct Authorized<A: ActionMarker, E: EntityTrait>(E::Model, PhantomData<fn() -> A>);

impl<A: ActionMarker, E: EntityTrait> Authorized<A, E> {
    /// Crate-private: only a seam passing through [`CrudService::access`] mints
    /// the proof, and the `graphql` bridge is the only one.
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

/// The entity's **read** API and the single audited gateway to the ORM. The
/// write half is the opt-in [`Creatable`], [`Updatable`] and [`Deletable`].
///
/// # Exposed by `#[crud]`
///
/// `#[crud]` on a controller or a resolver generates the operations the
/// service implements, each delegating here and declaring
/// `#[authorize(Action, Entity)]`. Over HTTP:
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::injectable;
/// use nest_rs_http::{controller, crud};
/// use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable};
/// # use nest_rs_core::{Discovery, module};
/// # use nest_rs_http::{HttpControllerMeta, HttpVerb};
/// # use nest_rs_testing::TestApp;
/// # mod users {
/// #     use nest_rs_resource::expose;
/// #     use sea_orm::entity::prelude::*;
/// #     #[expose(name = "User")]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
/// #     #[sea_orm(table_name = "users")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key, auto_increment = false)]
/// #         #[expose]
/// #         pub id: Uuid,
/// #         #[expose(input(create, update), validate(length(min = 1)))]
/// #         pub name: String,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
///
/// #[injectable]
/// #[derive(Default)]
/// struct UsersService;
///
/// impl CrudService for UsersService {
///     type Entity = users::Entity;
/// }
/// impl Creatable for UsersService {
///     type Create = users::CreateUser;
/// }
/// impl Updatable for UsersService {
///     type Update = users::UpdateUser;
/// }
/// impl Deletable for UsersService {}
///
/// #[controller(path = "/users")]
/// struct UsersController {
///     #[inject]
///     svc: Arc<UsersService>,
/// }
///
/// #[crud(
///     service = svc,
///     entity = users::Entity,
///     output = users::User,
///     create = users::CreateUser,
///     update = users::UpdateUser,
/// )]
/// impl UsersController {}
/// # #[module(providers = [UsersService, UsersController])]
/// # struct UsersModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = TestApp::for_module::<UsersModule>().await?;
/// # let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
/// # let routes: Vec<_> = controllers[0]
/// #     .meta
/// #     .routes
/// #     .iter()
/// #     .map(|route| (route.verb, route.path, route.handler))
/// #     .collect();
///
/// assert_eq!(
///     routes,
///     [
///         (HttpVerb::Get, "/", "list"),
///         (HttpVerb::Get, "/:id", "get"),
///         (HttpVerb::Post, "/", "create"),
///         (HttpVerb::Patch, "/:id", "update"),
///         (HttpVerb::Delete, "/:id", "delete"),
///     ],
/// );
/// # Ok(())
/// # }
/// ```
///
/// Over GraphQL:
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_graphql::{crud, resolver};
/// # use nest_rs_core::{injectable, module};
/// # use nest_rs_graphql::{GraphqlConfig, GraphqlModule};
/// # use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable};
/// # use nest_rs_testing::TestApp;
/// # mod users {
/// #     use nest_rs_resource::expose;
/// #     use sea_orm::entity::prelude::*;
/// #     #[expose(name = "User", graphql)]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
/// #     #[sea_orm(table_name = "users")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key, auto_increment = false)]
/// #         #[expose]
/// #         pub id: Uuid,
/// #         #[expose(input(create, update), validate(length(min = 1)))]
/// #         pub name: String,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # #[injectable]
/// # #[derive(Default)]
/// # struct UsersService;
/// #
/// # impl CrudService for UsersService {
/// #     type Entity = users::Entity;
/// # }
/// # impl Creatable for UsersService {
/// #     type Create = users::CreateUser;
/// # }
/// # impl Updatable for UsersService {
/// #     type Update = users::UpdateUser;
/// # }
/// # impl Deletable for UsersService {}
///
/// #[resolver]
/// struct UsersResolver {
///     #[inject]
///     svc: Arc<UsersService>,
/// }
///
/// #[crud(
///     service = svc,
///     entity = users::Entity,
///     output = users::User,
///     create = users::CreateUser,
///     update = users::UpdateUser,
/// )]
/// impl UsersResolver {}
/// # #[module(
/// #     imports = [GraphqlModule::for_root(GraphqlConfig {
/// #         disable_introspection: false,
/// #         ..GraphqlConfig::default()
/// #     })],
/// #     providers = [UsersService, UsersResolver],
/// # )]
/// # struct AppModule;
/// # async fn fields(app: &TestApp, root: &str) -> Vec<String> {
/// #     let query = format!("{{ __type(name: \"{root}\") {{ fields {{ name }} }} }}");
/// #     let resp = app.http().post("/graphql").body_json(&serde_json::json!({ "query": query })).send().await;
/// #     let body: serde_json::Value = resp.json().await.value().deserialize();
/// #     body["data"]["__type"]["fields"].as_array().into_iter().flatten()
/// #         .filter_map(|field| field["name"].as_str().map(str::to_owned)).collect()
/// # }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = TestApp::for_module::<AppModule>().await?;
///
/// assert_eq!(fields(&app, "Query").await, ["users", "user"]);
/// assert_eq!(fields(&app, "Mutation").await, ["createUser", "updateUser", "deleteUser"]);
/// # Ok(())
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a resource service",
    label = "expected a service implementing `CrudService`",
    note = "`Bind<A, S>` and `bind::<A, S>` name the action FIRST and the service SECOND. If this type is an action marker, the two type parameters are swapped."
)]
#[async_trait]
pub trait CrudService: Send + Sync
where
    <Self::Entity as EntityTrait>::ActiveModel: ActiveModelBehavior + Send,
    <Self::Entity as EntityTrait>::Model:
        Send + Sync + IntoActiveModel<<Self::Entity as EntityTrait>::ActiveModel>,
{
    /// The SeaORM entity this service is the audited API for.
    type Entity: EntityTrait;

    /// The entity's table name, the `entity` field on every log.
    fn entity_name() -> &'static str {
        Self::Entity::default().table_name()
    }

    /// Soft-delete opt-in. Override on the service to return the entity's
    /// `deleted_at` column. `None` (default) ⇒ hard delete and unfiltered reads.
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

    /// Every row the caller may [`Read`](Action::Read), up to
    /// [`LIST_CAP`](crate::LIST_CAP) rows; a capped result logs a `warn`.
    /// Collections that may exceed it paginate with [`page`](CrudService::page).
    async fn list(&self) -> Result<Vec<<Self::Entity as EntityTrait>::Model>, DbErr> {
        use sea_orm::QuerySelect;
        tracing::debug!(target: crate::TARGET, entity = Self::entity_name(), "listing rows");
        let conn = Repo::<Self::Entity>::conn()?;
        let rows = Repo::<Self::Entity>::scoped(Action::Read)
            .filter(Self::live_read_filter())
            .limit(crate::LIST_CAP + 1)
            .all(&conn)
            .await?;
        let (rows, capped) = crate::page::split_overfetched(rows, crate::LIST_CAP);
        if capped {
            tracing::warn!(
                target: crate::TARGET,
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
        tracing::debug!(target: crate::TARGET, entity = Self::entity_name(), first, ?after, "paging rows");
        Repo::<Self::Entity>::page(first, after, Self::live_read_filter()).await
    }

    /// Load a row by id and authorize the caller for `action` on it, in SQL
    /// under `condition_for(action)`, so a relational rule holds too. A
    /// denied-but-existing row is [`Access::Denied`], not [`Access::Missing`].
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
        if let Some(model) = Repo::<Self::Entity>::unscoped_by_id(id)
            .filter(scope_for::<Self::Entity>(action))
            .filter(live.clone())
            .one(&conn)
            .await?
        {
            tracing::debug!(target: crate::TARGET, entity, %id, ?action, "access granted");
            return Ok(Access::Found(model));
        }
        let exists = Repo::<Self::Entity>::unscoped_by_id(id)
            .filter(live)
            .one(&conn)
            .await?
            .is_some();
        if exists {
            tracing::warn!(target: crate::TARGET, entity, %id, ?action, "access denied");
            Ok(Access::Denied)
        } else {
            Ok(Access::Missing)
        }
    }
}

/// Opt-in write capability: the resource accepts **inserts**.
#[async_trait]
pub trait Creatable: CrudService {
    /// The create-input DTO this resource accepts, lowered to an `ActiveModel`
    /// via [`CreateModel`].
    type Create: CreateModel<Self::Entity> + Send;

    /// Insert a row from a create-input DTO, atomically with its scope check:
    /// the fresh row is re-checked in SQL against `condition_for(Create)`, and
    /// one outside the caller's scope is [`DbErr::RecordNotInserted`] and never
    /// persists.
    async fn create(
        &self,
        input: Self::Create,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        self.create_from_active(input.into_active_model()).await
    }

    /// Insert a **prepared** `ActiveModel` through the same audited path as
    /// [`create`](Creatable::create), for a service stamping server-side
    /// columns first; a raw `ActiveModel::insert(&Repo::conn()?)` bypasses the
    /// ability check.
    async fn create_from_active(
        &self,
        active: <Self::Entity as EntityTrait>::ActiveModel,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let entity = Self::entity_name();
        // A local transaction (a SAVEPOINT inside the request's): a swallowed
        // `RecordNotInserted` must not leave the row to commit with the request.
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
                        target: crate::TARGET,
                        entity,
                        error = %nest_rs_core::error_message(&rollback_err),
                        "rollback of the create SAVEPOINT/transaction failed",
                    );
                }
                Err(err)
            }
        }
    }
}

/// Insert, then re-check the fresh row against `condition_for(Create)` in SQL
/// on the same connection.
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
            target: crate::TARGET,
            entity,
            id = ?model_pk::<E>(&model),
            action = ?Action::Create,
            "access denied — row outside the caller's scope",
        );
        return Err(DbErr::RecordNotInserted);
    }
    tracing::debug!(target: crate::TARGET, entity, id = ?model_pk::<E>(&model), "row created");
    Ok(model)
}

/// Opt-in write capability: the resource accepts **updates**.
#[async_trait]
pub trait Updatable: CrudService {
    /// The update-input DTO this resource accepts, applied to a loaded row via
    /// [`UpdateModel`].
    type Update: UpdateModel<Self::Entity> + Send;

    /// Apply an update-input DTO to a loaded row, in the request transaction.
    /// Ability-scoped by [`Repo::update`]: a row outside the caller's scope is
    /// never touched and surfaces as [`DbErr::RecordNotUpdated`].
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
                tracing::debug!(target: crate::TARGET, entity, ?id, "row updated");
                Ok(updated)
            }
            Err(DbErr::RecordNotUpdated) => {
                tracing::warn!(
                    target: crate::TARGET,
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
/// [`soft_delete_column`](CrudService::soft_delete_column) is set, soft).
#[async_trait]
pub trait Deletable: CrudService {
    /// Delete a loaded row, in the request transaction. Ability-scoped by
    /// [`Repo::delete`]: a row outside the caller's scope is
    /// [`DbErr::RecordNotFound`].
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
                    tracing::debug!(target: crate::TARGET, entity, ?id, "row soft-deleted");
                    Ok(())
                }
                Err(DbErr::RecordNotUpdated) => {
                    tracing::warn!(
                        target: crate::TARGET,
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
                        target: crate::TARGET,
                        entity,
                        ?id,
                        action = ?Action::Delete,
                        "access denied — row outside the caller's scope",
                    );
                    return Err(out_of_scope());
                }
                tracing::debug!(target: crate::TARGET, entity, ?id, "row deleted");
                Ok(())
            }
        }
    }
}

/// First primary-key column value of a model, for logs; `None` for a
/// primary-key-less entity (views, raw tables).
fn model_pk<E: EntityTrait>(model: &E::Model) -> Option<sea_orm::Value> {
    use sea_orm::{Iterable, ModelTrait, PrimaryKeyToColumn};
    let pk_col = E::PrimaryKey::iter().next()?.into_column();
    Some(model.get(pk_col))
}

/// The model's first primary key as a `Uuid`, for `#[crud]`'s `201`
/// `Location`; `None` when the entity has none, or one of another type.
pub fn model_uuid<E: EntityTrait>(model: &E::Model) -> Option<Uuid> {
    <Uuid as sea_orm::sea_query::ValueType>::try_from(model_pk::<E>(model)?).ok()
}

/// Equality over **all** of a model's primary-key columns, composite included.
fn pk_condition<E: EntityTrait>(model: &E::Model) -> Condition {
    use sea_orm::{ColumnTrait, Iterable, ModelTrait, PrimaryKeyToColumn};
    let mut cond = Condition::all();
    for pk in E::PrimaryKey::iter() {
        let col = pk.into_column();
        cond = cond.add(col.eq(model.get(col)));
    }
    cond
}
