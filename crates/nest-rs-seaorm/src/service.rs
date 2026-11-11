use std::any::TypeId;
use std::marker::PhantomData;

use async_trait::async_trait;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Condition;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ActiveValue, DbErr, EntityName, EntityTrait, IdenStatic,
    IntoActiveModel, Iterable, PrimaryKeyTrait, QueryFilter,
};

use nest_rs_authz::{Ability, Action, ActionMarker, current_ability};
use nest_rs_database::atomic;

use crate::column_default::fill_column_defaults;
use crate::error::AtomicDbError;
use crate::page::{Page, PageCursor};
use crate::repo::{
    Repo, ability_scope, barred_from_class, insert_contained, pk_condition, write_scope,
};
use crate::select::RepoSelect;

/// Build a fresh `ActiveModel` from a create-input DTO. Implemented by `#[expose]`
/// for each `Create<Name>Input`.
pub trait CreateModel<E: EntityTrait> {
    /// Turn the create-input DTO into a fresh `ActiveModel` ready to insert.
    fn into_active_model(self) -> E::ActiveModel;

    /// The field of this input that carries `column` (as the table names it),
    /// by the name serde reads it under, which validation's `errors` keys share:
    /// what a `#[crud]` create a unique key or a grant's `.fields` refuses names
    /// under `errors`. `None` for a column the input does not carry, and, unless an
    /// implementation says otherwise, for every column.
    ///
    /// ```
    /// # mod users {
    /// #     use nest_rs_seaorm::expose;
    /// #     use sea_orm::entity::prelude::*;
    /// #[expose(name = "User")]
    /// #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    /// #[sea_orm(table_name = "users")]
    /// pub struct Model {
    ///     #[sea_orm(primary_key, auto_increment = false)]
    ///     #[expose]
    ///     pub id: Uuid,
    ///     #[expose(input(create))]
    ///     #[sea_orm(column_name = "email_address")]
    ///     pub email: String,
    /// }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// use nest_rs_seaorm::CreateModel;
    ///
    /// assert_eq!(users::CreateUser::field_for("email_address"), Some("email"));
    /// assert_eq!(users::CreateUser::field_for("id"), None);
    /// ```
    fn field_for(column: &str) -> Option<&'static str> {
        let _ = column;
        None
    }
}

/// Apply an update-input DTO onto a loaded `ActiveModel`. Implemented by
/// `#[expose]` for each `Update<Name>Input`.
pub trait UpdateModel<E: EntityTrait> {
    /// Apply the update-input DTO's set fields onto the loaded `ActiveModel`,
    /// leaving unset fields untouched.
    fn apply_to(self, model: E::ActiveModel) -> E::ActiveModel;

    /// The field of this input that carries `column`, by the name serde reads it
    /// under, which validation's `errors` keys share, as
    /// [`CreateModel::field_for`] says it: what a `#[crud]` update a unique key or
    /// a grant's `.fields` refuses names under `errors`.
    fn field_for(column: &str) -> Option<&'static str> {
        let _ = column;
        None
    }

    /// Whether this input carries `column` (as the table names it) and the wire
    /// shows it, so a caller holds the value it would send back: under a grant
    /// narrowing what it may write, [`Updatable::update`] reads such a column left
    /// as it stands as no write. `false` unless an implementation says otherwise,
    /// and for any column no reply shows, whose stored value a refusal must not
    /// tell apart.
    fn reads_back(column: &str) -> bool {
        let _ = column;
        false
    }
}

/// Outcome of an authorized by-id load: 200/403/404 on REST, data/forbidden/null
/// on GraphQL.
///
/// Exhaustive on purpose: found, denied or missing is the whole answer to a
/// load by id, so a caller matches all three and never needs a `_` arm.
#[expect(
    clippy::exhaustive_enums,
    reason = "closed by definition: a load by id finds the row, is denied it, or misses it"
)]
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
///
/// An HTTP route, a GraphQL operation or an MCP `#[resource]` receives it as a
/// parameter when it declares `#[authorize(A, bind = S)]`: the attribute is the
/// posture, and this type carries the row it loaded, nothing more.
pub struct Authorized<A: ActionMarker, E: EntityTrait>(E::Model, PhantomData<fn() -> A>);

impl<A: ActionMarker, E: EntityTrait> Authorized<A, E> {
    /// Crate-private: only a seam passing through [`CrudService::access`] mints
    /// the proof — `#[authorize(A, bind = S)]`'s loader on each edge.
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
/// # use nest_rs_http::{HttpControllerMeta, Method};
/// # use nest_rs_testing::HeadlessApp;
/// # mod users {
/// #     use nest_rs_seaorm::expose;
/// #     use sea_orm::entity::prelude::*;
/// #     #[expose(name = "User")]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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
/// #[crud(service = UsersService)]
/// impl UsersController {}
/// # #[module(
/// #     imports = [nest_rs_seaorm::SeaOrmDatabaseModule],
/// #     providers = [UsersService, UsersController],
/// # )]
/// # struct UsersModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = HeadlessApp::builder()
/// #     .provide(sea_orm::DatabaseConnection::default())
/// #     .provide(nest_rs_seaorm::SeaOrmConfig::default())
/// #     .module::<UsersModule>()
/// #     .build_headless()
/// #     .await?;
/// # let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
/// # let routes: Vec<_> = controllers[0]
/// #     .meta
/// #     .routes()
/// #     .iter()
/// #     .map(|route| (route.method().clone(), route.path(), route.handler()))
/// #     .collect();
///
/// assert_eq!(
///     routes,
///     [
///         (Method::GET, "/", "list"),
///         (Method::GET, "/{id}", "get"),
///         (Method::POST, "/", "create"),
///         (Method::PUT, "/{id}", "update"),
///         (Method::DELETE, "/{id}", "delete"),
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
/// #     use nest_rs_seaorm::expose;
/// #     use sea_orm::entity::prelude::*;
/// #     #[expose(name = "User", graphql)]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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
/// # #[derive(Clone)]
/// # struct Caller;
/// # impl nest_rs_core::AmbientValue for Caller {}
/// # #[injectable]
/// # #[derive(Default)]
/// # struct AuthzAbility;
/// # impl nest_rs_authz::AbilityFactory for AuthzAbility {
/// #     type Actor = Caller;
/// #     fn define(&self, _actor: &Caller, _ability: &mut nest_rs_authz::AbilityBuilder) {}
/// # }
/// # type AuthzGuard = nest_rs_authz::AbilityGuard<AuthzAbility>;
///
/// #[resolver]
/// #[use_guards(AuthzGuard)]
/// struct UsersResolver {
///     #[inject]
///     svc: Arc<UsersService>,
/// }
///
/// #[crud(service = UsersService)]
/// impl UsersResolver {}
/// # #[module(
/// #     imports = [
/// #         GraphqlModule::for_root(GraphqlConfig {
/// #             introspection: nest_rs_config::Surface::Open,
/// #             ..GraphqlConfig::default()
/// #         }),
/// #         nest_rs_seaorm::SeaOrmDatabaseModule,
/// #     ],
/// #     providers = [
/// #         UsersService,
/// #         AuthzAbility,
/// #         AuthzGuard,
/// #         nest_rs_http::EndpointGuards<nest_rs_graphql::GraphqlView, (AuthzGuard,)>,
/// #         UsersResolver,
/// #     ],
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
/// # let app = TestApp::builder()
/// #     .provide(sea_orm::DatabaseConnection::default())
/// #     .provide(nest_rs_seaorm::SeaOrmConfig::default())
/// #     .module::<AppModule>()
/// #     .build()
/// #     .await?;
///
/// assert_eq!(fields(&app, "Query").await, ["users", "user"]);
/// assert_eq!(fields(&app, "Mutation").await, ["createUser", "updateUser", "deleteUser"]);
/// # Ok(())
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a resource service",
    label = "expected a service implementing `CrudService`",
    note = "`#[crud(service = Service)]` and `#[authorize(Action, bind = Service)]` name a type implementing `CrudService`; beside `bind =`, the action comes first, so an action marker there is the two swapped"
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

    /// Every row the caller may [`Read`](Action::Read), up to
    /// [`LIST_CAP`](crate::LIST_CAP) rows; a capped result logs a `warn`.
    /// Collections that may exceed it paginate with [`page`](CrudService::page).
    async fn list(&self) -> Result<Vec<<Self::Entity as EntityTrait>::Model>, DbErr> {
        tracing::debug!(target: crate::target::ORM, entity = Self::entity_name(), "listing rows");
        let rows = Repo::<Self::Entity>::scoped(Action::Read)
            .limit(crate::LIST_CAP + 1)
            .all()
            .await?;
        let (rows, capped) = crate::page::split_overfetched(rows, crate::LIST_CAP);
        if capped {
            tracing::warn!(
                target: crate::target::ORM,
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
        after: Option<PageCursor>,
    ) -> Result<Page<<Self::Entity as EntityTrait>::Model>, DbErr>
    where
        <Self::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
        <Self::Entity as EntityTrait>::Model: Send + Sync,
    {
        tracing::debug!(target: crate::target::ORM, entity = Self::entity_name(), first, after = after.as_ref().map(tracing::field::display), "paging rows");
        Repo::<Self::Entity>::page(first, after, Condition::all()).await
    }

    /// Load a row by id and authorize the caller for `action` on it, in SQL
    /// under `condition_for(action)`, so a relational rule holds too. A
    /// denied-but-existing row is [`Access::Denied`], not [`Access::Missing`];
    /// a soft-deleted row is [`Access::Missing`], whatever the action.
    async fn access(
        &self,
        action: Action,
        id: Uuid,
    ) -> Result<Access<<Self::Entity as EntityTrait>::Model>, DbErr>
    where
        <Self::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    {
        let entity = Self::entity_name();
        if let Some(model) = Repo::<Self::Entity>::unscoped_by_id(id)
            .within(ability_scope::<Self::Entity>(action))
            .one()
            .await?
        {
            tracing::debug!(target: crate::target::ORM, entity, %id, action = action.as_str(), "access granted");
            return Ok(Access::Found(model));
        }
        let exists = Repo::<Self::Entity>::unscoped_by_id(id).exists().await?;
        if exists {
            tracing::warn!(target: crate::target::ORM, entity, %id, action = action.as_str(), "access denied");
            Ok(Access::Denied)
        } else {
            Ok(Access::Missing)
        }
    }
}

/// The entity a bound service loads, held by its one impl to the one the
/// operation names — written in `#[authorize(A, E, bind = S)]`, or read off its
/// `Authorized<A, E>` parameter — which the class gate reads.
#[diagnostic::on_unimplemented(
    message = "the operation names `{E}`, and the service `bind` names loads `{Self}`",
    label = "the service loads another entity",
    note = "the class gate and the `Authorized<Action, E>` parameter read the entity the operation names, the load reads the service's: name `{Self}` in both, or bind the service loading `{E}`"
)]
pub trait SameEntity<E> {}

impl<E> SameEntity<E> for E {}

/// Compiles only when `S` loads `E`, asserted where both are written.
pub const fn bound_entity<S, E>()
where
    S: CrudService,
    S::Entity: SameEntity<E>,
{
}

/// Opt-in write capability: the resource accepts **inserts**.
#[diagnostic::on_unimplemented(
    message = "`{Self}` offers no create: implement `Creatable` for it, or drop `create` from `#[crud(ops = …)]`",
    label = "`#[crud]` generates a create for this service",
    note = "`#[crud]` generates every op `ops` lists, all five by default, each typed off the service's trait: `impl Creatable for {Self}` declares the input it inserts"
)]
#[async_trait]
pub trait Creatable: CrudService {
    /// The create-input DTO this resource accepts, lowered to an `ActiveModel`
    /// via [`CreateModel`].
    type Create: CreateModel<Self::Entity> + Send;

    /// Insert a row from a create-input DTO, atomically with its scope check.
    ///
    /// A column the input leaves unset takes the value the ambient ability's
    /// `Create` grants pin it to ([`Ability::pinned_values`](nest_rs_authz::Ability::pinned_values)):
    /// the tenant a grant scopes the caller to, the author it names; one no
    /// grant pins takes the default its `#[expose]` entity declares. The
    /// fresh row is then re-checked in SQL against `condition_for(Create)`, and
    /// one outside the caller's scope is [`DbErr::RecordNotInserted`] and never
    /// persists — so a value the grants leave undecided is refused, never
    /// guessed. A caller no row could pass for — no `Create` grant, a denial no
    /// `when` narrows ([`Ability::can_class`]), or a request holding no ability,
    /// as on a route whose posture names no subject — is refused the same way
    /// before the hook runs or the insert is sent.
    ///
    /// A `Create` grant's `.fields` limits what the input sets, never what a pin,
    /// a default or a hook fills, nor the key: a column no grant permits is
    /// [`DbErr::AccessDenied`] naming it, before anything runs, and the re-check
    /// holds the fresh row to a grant permitting each column.
    async fn create(
        &self,
        input: Self::Create,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        self.create_from_active(input.into_active_model()).await
    }

    /// Insert a **prepared** `ActiveModel` through the same audited path as
    /// [`create`](Creatable::create), its pinned columns filled alike, for a
    /// service setting a column neither the input nor the grants decide.
    async fn create_from_active(
        &self,
        active: <Self::Entity as EntityTrait>::ActiveModel,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let entity = Self::entity_name();
        Repo::<Self::Entity>::conn()?;
        // No row can pass the re-check, so the insert is never sent: what the
        // database would refuse first would answer in its place.
        if barred_from_class::<Self::Entity>(Action::Create) {
            tracing::warn!(
                target: crate::target::ORM,
                entity,
                action = Action::Create.as_str(),
                "access denied — no row of the entity is in the caller's scope",
            );
            return Err(DbErr::RecordNotInserted);
        }
        let scope = write_scope(Action::Create, &active)?;
        let active = match current_ability() {
            Some(ability) => fill_pinned::<Self::Entity>(active, &ability, entity),
            None => active,
        };
        // A swallowed `RecordNotInserted` must not leave the row to commit with
        // the request.
        atomic(async move {
            Ok::<_, AtomicDbError>(insert_in_scope::<Self::Entity>(active, scope, entity).await?)
        })
        .await
        .map_err(DbErr::from)
    }
}

/// Set each column `ability`'s `Create` grants pin that `active` leaves unset.
fn fill_pinned<E: EntityTrait>(
    mut active: E::ActiveModel,
    ability: &Ability,
    entity: &'static str,
) -> E::ActiveModel {
    let mut filled = Vec::new();
    for (column, value) in ability.pinned_values::<E>(Action::Create) {
        if !active.is_not_set(column) {
            continue;
        }
        match active.try_set(column, value) {
            Ok(()) => filled.push(column.as_str()),
            Err(err) => tracing::warn!(
                target: crate::target::ORM,
                entity,
                column = column.as_str(),
                error = %nest_rs_core::error_message(&err),
                "a value the ability pins does not fit its column — left unset",
            ),
        }
    }
    if !filled.is_empty() {
        tracing::debug!(target: crate::target::ORM, entity, columns = filled.join(","), "create filled from the ability's grants");
    }
    active
}

/// Fill the entity's column defaults the row leaves unset, after the grants'
/// pins so a pin wins over a default, then insert and re-check the fresh row
/// against `scope` in SQL on the same executor.
async fn insert_in_scope<E>(
    mut active: E::ActiveModel,
    scope: Condition,
    entity: &'static str,
) -> Result<E::Model, DbErr>
where
    E: EntityTrait,
    E::ActiveModel: ActiveModelBehavior + Send,
    E::Model: IntoActiveModel<E::ActiveModel>,
{
    fill_column_defaults::<E>(&mut active);
    let model = insert_contained::<E>(active, &Repo::<E>::conn()?).await?;
    let in_scope = RepoSelect::new(E::find().filter(scope))
        .within(pk_condition::<E>(&model))
        .exists()
        .await?;
    if !in_scope {
        tracing::warn!(
            target: crate::target::ORM,
            entity,
            id = line_id::<E>(&model),
            action = Action::Create.as_str(),
            "access denied — row outside the caller's scope",
        );
        return Err(DbErr::RecordNotInserted);
    }
    tracing::debug!(target: crate::target::ORM, entity, id = line_id::<E>(&model), "row created");
    Ok(model)
}

/// Opt-in write capability: the resource accepts **updates**.
#[diagnostic::on_unimplemented(
    message = "`{Self}` offers no update: implement `Updatable` for it, or drop `update` from `#[crud(ops = …)]`",
    label = "`#[crud]` generates an update for this service",
    note = "`#[crud]` generates every op `ops` lists, all five by default, each typed off the service's trait: `impl Updatable for {Self}` declares the input it applies"
)]
#[async_trait]
pub trait Updatable: CrudService {
    /// The update-input DTO this resource accepts, applied to a loaded row via
    /// [`UpdateModel`].
    type Update: UpdateModel<Self::Entity> + Send;

    /// Apply an update-input DTO to a loaded row, in the request transaction.
    /// Ability-scoped by [`Repo::update`]: a row outside the caller's scope is
    /// never touched and surfaces as [`DbErr::RecordNotUpdated`].
    ///
    /// An input replaces the row's writable state (RFC 9110 §9.3.4), so under a
    /// grant whose `.fields` narrows what the caller may write, a column it carries
    /// back as it stands is no write, where the caller may read it: it stays out of
    /// the statement. Any other column it sets is held to the grant, one it may not
    /// read whatever its value, so a refusal tells nothing of what the row holds.
    async fn update(
        &self,
        model: <Self::Entity as EntityTrait>::Model,
        input: Self::Update,
    ) -> Result<<Self::Entity as EntityTrait>::Model, DbErr> {
        let entity = Self::entity_name();
        let id = line_id::<Self::Entity>(&model);
        let active = applied::<Self::Entity, Self::Update>(model, input);
        match Repo::<Self::Entity>::update(active).await {
            Ok(updated) => {
                tracing::debug!(target: crate::target::ORM, entity, id, "row updated");
                Ok(updated)
            }
            Err(DbErr::RecordNotUpdated) => {
                tracing::warn!(
                    target: crate::target::ORM,
                    entity,
                    id,
                    action = Action::Update.as_str(),
                    "access denied — row outside the caller's scope",
                );
                Err(DbErr::RecordNotUpdated)
            }
            Err(err) => Err(err),
        }
    }
}

/// `input` applied to `model`. Under a grant narrowing what the caller may write, a
/// column the input carries back as it stands stays [`Unchanged`](ActiveValue::Unchanged)
/// where the caller may read it, by what [`Ability::can`] and its field list say of the
/// row itself: a relational `Read` grant, which only SQL decides, lets nothing through.
fn applied<E, I>(model: E::Model, input: I) -> E::ActiveModel
where
    E: EntityTrait,
    E::Model: IntoActiveModel<E::ActiveModel>,
    I: UpdateModel<E>,
{
    let Some(ability) = current_ability()
        .filter(|ability| !ability.grants_every_column(Action::Update, TypeId::of::<E>()))
    else {
        return input.apply_to(model.into_active_model());
    };
    let readable = ability
        .can::<E>(Action::Read, &model)
        .then(|| ability.permitted_fields::<E>(Action::Read, &model));
    let set = input.apply_to(model.clone().into_active_model());
    let mut written = model.into_active_model();
    for column in E::Column::iter() {
        let name = column.as_str();
        match set.get(column) {
            ActiveValue::Set(value)
                if I::reads_back(name) && readable.as_ref().is_some_and(|r| r.permits(name)) =>
            {
                written.set_if_not_equals(column, value);
            }
            ActiveValue::Set(value) => written.set(column, value),
            ActiveValue::Unchanged(_) => {}
            ActiveValue::NotSet => written.not_set(column),
        }
    }
    written
}

/// Opt-in write capability: the resource accepts **deletes**: a soft delete
/// when the entity is declared `#[expose(..., soft_delete)]`, else a hard one.
#[diagnostic::on_unimplemented(
    message = "`{Self}` offers no delete: implement `Deletable` for it, or drop `delete` from `#[crud(ops = …)]`",
    label = "`#[crud]` generates a delete for this service",
    note = "`#[crud]` generates every op `ops` lists, all five by default, each typed off the service's trait: `impl Deletable for {Self}` opens its deletes"
)]
#[async_trait]
pub trait Deletable: CrudService {
    /// Delete a loaded row, in the request transaction. Ability-scoped by
    /// [`Repo::delete`]: a row outside the caller's scope is
    /// [`DbErr::RecordNotFound`].
    async fn delete(&self, model: <Self::Entity as EntityTrait>::Model) -> Result<(), DbErr> {
        let entity = Self::entity_name();
        let id = line_id::<Self::Entity>(&model);
        let out_of_scope = || {
            DbErr::RecordNotFound(format!(
                "{entity} row not found or outside the caller's scope"
            ))
        };
        match crate::soft_delete::tombstone_column::<Self::Entity>() {
            Some(col) => match Repo::<Self::Entity>::soft_delete(model, col).await {
                Ok(()) => {
                    tracing::debug!(target: crate::target::ORM, entity, id, "row soft-deleted");
                    Ok(())
                }
                Err(DbErr::RecordNotUpdated) => {
                    tracing::warn!(
                        target: crate::target::ORM,
                        entity,
                        id,
                        action = Action::Delete.as_str(),
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
                        target: crate::target::ORM,
                        entity,
                        id,
                        action = Action::Delete.as_str(),
                        "access denied — row outside the caller's scope",
                    );
                    return Err(out_of_scope());
                }
                tracing::debug!(target: crate::target::ORM, entity, id, "row deleted");
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

/// The model's first primary key as a line says it, written only if a line is
/// filed; absent for a keyless entity.
fn line_id<E: EntityTrait>(model: &E::Model) -> Option<tracing::field::DisplayValue<LineKey>> {
    model_pk::<E>(model).map(|key| tracing::field::display(LineKey(key)))
}

/// A key's bare value, as a by-id line's `%id` prints it; `Value`'s own `Display`
/// writes SQL, which quotes a `Uuid` and a string.
struct LineKey(sea_orm::Value);

impl std::fmt::Display for LineKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            sea_orm::Value::Uuid(Some(key)) => key.fmt(f),
            sea_orm::Value::String(Some(key)) => f.write_str(key),
            key => key.fmt(f),
        }
    }
}

/// The model's first primary key as a `Uuid`; `None` when the entity has
/// none, or one of another type.
pub fn model_uuid<E: EntityTrait>(model: &E::Model) -> Option<Uuid> {
    <Uuid as sea_orm::sea_query::ValueType>::try_from(model_pk::<E>(model)?).ok()
}

/// The key of a row `#[crud]`'s create inserted, which its `201` names: a
/// `CrudService` entity is keyed by a `Uuid`, as its by-id reads are.
pub fn created_key<E>(model: &E::Model) -> Uuid
where
    E: EntityTrait,
    E::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
{
    #[expect(
        clippy::expect_used,
        reason = "the bound makes the one key a `Uuid`, which its column value reads back as"
    )]
    model_uuid::<E>(model).expect("a `Uuid`-keyed row reads its key back as a `Uuid`")
}

#[cfg(test)]
mod tests {
    use nest_rs_authz::{Ability, AbilityBuilder};
    use sea_orm::{ActiveValue, Set};

    use super::*;

    mod note {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "notes")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
            pub org_id: Uuid,
            pub author_id: Uuid,
            pub body: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    fn pinning(org: Uuid, author: Uuid) -> Ability {
        let mut b = AbilityBuilder::new();
        b.can(Action::Create, note::Entity).when(|p| {
            p.all([
                p.eq(note::Column::OrgId, org),
                p.eq(note::Column::AuthorId, author),
            ])
        });
        b.build().expect("a valid ability")
    }

    #[test]
    fn a_create_fills_what_the_ability_pins_and_the_input_left_unset() {
        let (org, author, written) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let active = note::ActiveModel {
            author_id: Set(written),
            body: Set("hello".into()),
            ..Default::default()
        };

        let filled = fill_pinned::<note::Entity>(active, &pinning(org, author), "notes");

        assert_eq!(filled.org_id, Set(org));
        assert_eq!(
            filled.author_id,
            Set(written),
            "a value the input carries is the scope check's to judge, never overwritten",
        );
        assert_eq!(filled.id, ActiveValue::NotSet);
    }

    #[test]
    fn a_create_says_each_column_it_filled_by_its_name() {
        let logs = nest_rs_testing::LogCapture::install();

        fill_pinned::<note::Entity>(
            <note::ActiveModel as Default>::default(),
            &pinning(Uuid::now_v7(), Uuid::now_v7()),
            "notes",
        );

        let line = logs.expect_one(
            crate::target::ORM,
            "create filled from the ability's grants",
        );
        assert_eq!(line.field("entity").as_deref(), Some("notes"));
        assert_eq!(line.field("columns").as_deref(), Some("org_id,author_id"));
    }

    #[test]
    fn a_pinned_value_its_column_cannot_hold_is_left_unset_and_said() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut b = AbilityBuilder::new();
        b.can(Action::Create, note::Entity)
            .when(|p| p.eq(note::Column::OrgId, "not-a-uuid"));
        let ability = b.build().expect("a valid ability");

        let filled = fill_pinned::<note::Entity>(
            <note::ActiveModel as Default>::default(),
            &ability,
            "notes",
        );

        assert_eq!(filled.org_id, ActiveValue::NotSet);
        let event = logs.expect_one(
            crate::target::ORM,
            "a value the ability pins does not fit its column — left unset",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("column").as_deref(), Some("org_id"));
        assert!(!event.quotes("not-a-uuid"), "{event:?}");
    }

    struct NotesService;

    impl CrudService for NotesService {
        type Entity = note::Entity;
    }

    struct CreateNote;

    impl CreateModel<note::Entity> for CreateNote {
        fn into_active_model(self) -> note::ActiveModel {
            note::ActiveModel {
                id: Set(Uuid::now_v7()),
                body: Set("hello".into()),
                ..Default::default()
            }
        }
    }

    impl Creatable for NotesService {
        type Create = CreateNote;
    }

    /// On a pool no statement can reach, so a create that tried one would
    /// answer its connection error.
    #[tokio::test]
    async fn a_create_a_denial_bars_outright_runs_no_statement() {
        use std::sync::Arc;

        use nest_rs_authz::__private::install_ability;
        use nest_rs_database::__private::with_request_executor;
        use sea_orm::DatabaseConnection;

        let logs = nest_rs_testing::LogCapture::install();
        let mut b = AbilityBuilder::new();
        b.can(Action::Manage, note::Entity)
            .when(|p| p.eq(note::Column::OrgId, Uuid::now_v7()));
        b.cannot(Action::Create, note::Entity);
        let ability = Arc::new(b.build().expect("a valid ability"));

        let created = with_request_executor(
            crate::executor::SeaOrmExecutor::Pool(DatabaseConnection::default()).into(),
            install_ability(ability, NotesService.create(CreateNote)),
        )
        .await;

        assert!(
            matches!(created, Err(DbErr::RecordNotInserted)),
            "the scope refusal, never the database's: {created:?}",
        );
        let event = logs.expect_one(
            crate::target::ORM,
            "access denied — no row of the entity is in the caller's scope",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("entity").as_deref(), Some("notes"));
    }

    /// `.fields` on `Create` names what the input may write, on a pool no statement
    /// can reach; the key it seeds and what the grant pins are no write of its own.
    #[tokio::test]
    async fn a_create_of_a_column_its_grant_leaves_out_runs_no_statement() {
        let org = Uuid::now_v7();
        let mut b = AbilityBuilder::new();
        b.can(Action::Create, note::Entity)
            .when(|p| p.eq(note::Column::OrgId, org))
            .fields([note::Column::AuthorId]);
        let ability = b.build().expect("a valid ability");

        let refused = on_no_pool(ability, NotesService.create(CreateNote)).await;

        assert!(
            matches!(
                &refused,
                Err(DbErr::AccessDenied { permission, resource })
                    if permission == "insert" && resource == "notes(body)"
            ),
            "the column by name, never the database's refusal: {refused:?}",
        );
    }

    struct EditNote {
        org_id: Uuid,
        body: String,
    }

    impl UpdateModel<note::Entity> for EditNote {
        fn apply_to(self, mut model: note::ActiveModel) -> note::ActiveModel {
            model.org_id = Set(self.org_id);
            model.body = Set(self.body);
            model
        }

        fn reads_back(column: &str) -> bool {
            ["org_id", "body"].contains(&column)
        }
    }

    impl Updatable for NotesService {
        type Update = EditNote;
    }

    /// The same input, hand-written with no `reads_back`: nothing says a reply shows
    /// what it sets.
    struct EditUnshown(EditNote);

    impl UpdateModel<note::Entity> for EditUnshown {
        fn apply_to(self, model: note::ActiveModel) -> note::ActiveModel {
            self.0.apply_to(model)
        }
    }

    /// `work` under `ability`, on a pool no statement can reach.
    async fn on_no_pool<T>(ability: Ability, work: impl Future<Output = T>) -> T {
        use std::sync::Arc;

        use nest_rs_authz::__private::install_ability;
        use nest_rs_database::__private::with_request_executor;
        use sea_orm::DatabaseConnection;

        with_request_executor(
            crate::executor::SeaOrmExecutor::Pool(DatabaseConnection::default()).into(),
            install_ability(Arc::new(ability), work),
        )
        .await
    }

    /// What `input` sets on `row`, under `ability`.
    async fn applied_under<I: UpdateModel<note::Entity>>(
        ability: Ability,
        row: note::Model,
        input: I,
    ) -> note::ActiveModel {
        on_no_pool(ability, async { applied::<note::Entity, I>(row, input) }).await
    }

    fn a_note() -> note::Model {
        note::Model {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            author_id: Uuid::now_v7(),
            body: "as it stands".to_owned(),
        }
    }

    /// Writes `body` alone, and reads the columns `readable` names.
    fn writing_the_body(readable: Option<&[note::Column]>) -> Ability {
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, note::Entity)
            .fields([note::Column::Body]);
        if let Some(columns) = readable {
            b.can(Action::Read, note::Entity)
                .fields(columns.iter().copied());
        }
        b.build().expect("a valid ability")
    }

    /// RFC 9110 §9.3.4: a replacement carries the row's writable state, so a caller
    /// narrowed to `body` sends `org_id` back as it read it. It stays out of the
    /// statement, where nothing it wrote could hold the row's next writer back.
    #[tokio::test]
    async fn an_input_carrying_a_column_back_as_it_stands_does_not_write_it() {
        let row = a_note();
        let input = EditNote {
            org_id: row.org_id,
            body: "edited".to_owned(),
        };

        let written = applied_under(
            writing_the_body(Some(&[note::Column::OrgId, note::Column::Body])),
            row.clone(),
            input,
        )
        .await;

        assert_eq!(written.org_id, ActiveValue::Unchanged(row.org_id));
        assert_eq!(written.body, Set("edited".to_owned()));
        assert_eq!(written.id, ActiveValue::Unchanged(row.id));
    }

    #[tokio::test]
    async fn an_input_changing_a_column_writes_it_so_the_grant_refuses_it() {
        let row = a_note();
        let moved = Uuid::now_v7();
        let input = EditNote {
            org_id: moved,
            body: row.body.clone(),
        };

        let written = applied_under(
            writing_the_body(Some(&[note::Column::OrgId, note::Column::Body])),
            row.clone(),
            input,
        )
        .await;

        assert_eq!(written.org_id, Set(moved));
        assert_eq!(written.body, ActiveValue::Unchanged(row.body));
    }

    /// What `applied` writes `Repo::update` refuses by name, on a pool no statement can
    /// reach: a change of a column the caller reads, and a column it may not read,
    /// whose right guess and wrong one hear the same.
    #[tokio::test]
    async fn an_update_setting_a_withheld_column_runs_no_statement() {
        let row = a_note();
        let reads_it: &[note::Column] = &[note::Column::OrgId, note::Column::Body];
        let reads_the_body: &[note::Column] = &[note::Column::Body];
        for (readable, org_id) in [
            (reads_it, Uuid::now_v7()),
            (reads_the_body, row.org_id),
            (reads_the_body, Uuid::now_v7()),
        ] {
            let input = EditNote {
                org_id,
                body: "edited".to_owned(),
            };

            let refused = on_no_pool(
                writing_the_body(Some(readable)),
                NotesService.update(row.clone(), input),
            )
            .await;

            assert!(
                matches!(
                    &refused,
                    Err(DbErr::AccessDenied { permission, resource })
                        if permission == "update" && resource == "notes(org_id)"
                ),
                "{refused:?}",
            );
        }
    }

    /// Comparing what a caller may not read would tell it what the row holds, so
    /// the column counts as written whatever its value: a refusal says nothing.
    #[tokio::test]
    async fn a_column_the_caller_may_not_read_is_written_whatever_its_value() {
        for readable in [Some(&[note::Column::Body][..]), None] {
            let row = a_note();
            let input = EditNote {
                org_id: row.org_id,
                body: row.body.clone(),
            };

            let written = applied_under(writing_the_body(readable), row.clone(), input).await;

            assert_eq!(written.org_id, Set(row.org_id), "{readable:?}");
        }
    }

    /// A grant the row alone cannot decide reads nothing here: this comparison may
    /// be the only read of a row its `Read` grant does not admit.
    #[tokio::test]
    async fn a_read_grant_the_row_does_not_meet_reads_nothing() {
        let row = a_note();
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, note::Entity)
            .fields([note::Column::Body]);
        let other_org = Uuid::now_v7();
        b.can(Action::Read, note::Entity)
            .when(|p| p.eq(note::Column::OrgId, other_org));
        let input = EditNote {
            org_id: row.org_id,
            body: "edited".to_owned(),
        };

        let written = applied_under(b.build().expect("a valid ability"), row.clone(), input).await;

        assert_eq!(written.org_id, Set(row.org_id));
    }

    /// A column no reply shows is the same: an input that sets a hidden column
    /// compares nothing, a `Read` grant with no list notwithstanding.
    #[tokio::test]
    async fn a_column_no_reply_shows_is_written_whatever_its_value() {
        let row = a_note();
        let input = EditUnshown(EditNote {
            org_id: row.org_id,
            body: "edited".to_owned(),
        });
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, note::Entity)
            .fields([note::Column::Body]);
        b.can(Action::Read, note::Entity);

        let written = applied_under(b.build().expect("a valid ability"), row.clone(), input).await;

        assert_eq!(written.org_id, Set(row.org_id));
    }

    /// No `.fields` on a write, no change: every member the input carries is set.
    #[tokio::test]
    async fn a_grant_with_no_field_list_sets_all_the_input_carries() {
        let row = a_note();
        let input = EditNote {
            org_id: row.org_id,
            body: row.body.clone(),
        };
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, note::Entity);
        b.can(Action::Read, note::Entity);

        let written = applied_under(b.build().expect("a valid ability"), row.clone(), input).await;

        assert_eq!(written.org_id, Set(row.org_id));
        assert_eq!(written.body, Set(row.body));
    }

    /// A request no ability reaches — a route whose posture names no subject —
    /// is refused as one its ability bars, on a pool no statement can reach.
    #[tokio::test]
    async fn a_create_in_a_request_holding_no_ability_runs_no_statement() {
        use nest_rs_database::__private::with_request_executor;
        use sea_orm::DatabaseConnection;

        let logs = nest_rs_testing::LogCapture::install();

        let created = with_request_executor(
            crate::executor::SeaOrmExecutor::Pool(DatabaseConnection::default()).into(),
            NotesService.create(CreateNote),
        )
        .await;

        assert!(
            matches!(created, Err(DbErr::RecordNotInserted)),
            "the scope refusal, never the database's: {created:?}",
        );
        let said = logs.expect_one(
            crate::target::ORM,
            "no ambient Ability outside a worker job — denying all rows",
        );
        assert_eq!(said.level, "warn");
        assert_eq!(said.field("action").as_deref(), Some("create"));
    }

    mod counter {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "counters")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i64,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    struct CountersService;

    impl CrudService for CountersService {
        type Entity = counter::Entity;
    }

    struct Touch;

    impl UpdateModel<counter::Entity> for Touch {
        fn apply_to(self, model: counter::ActiveModel) -> counter::ActiveModel {
            model
        }
    }

    impl Updatable for CountersService {
        type Update = Touch;
    }

    #[tokio::test]
    async fn a_refused_write_names_a_row_keyed_by_no_uuid() {
        use nest_rs_database::__private::with_request_executor;
        use sea_orm::DatabaseConnection;

        let logs = nest_rs_testing::LogCapture::install();

        let updated = with_request_executor(
            crate::executor::SeaOrmExecutor::Pool(DatabaseConnection::default()).into(),
            CountersService.update(counter::Model { id: 5 }, Touch),
        )
        .await;

        assert!(
            matches!(updated, Err(DbErr::RecordNotUpdated)),
            "{updated:?}"
        );
        let line = logs.expect_one(
            crate::target::ORM,
            "access denied — row outside the caller's scope",
        );
        assert_eq!(line.field("entity").as_deref(), Some("counters"));
        assert_eq!(line.field("id").as_deref(), Some("5"));
        assert_eq!(line.field("action").as_deref(), Some("update"));
    }

    #[test]
    fn a_key_reads_as_its_bare_value() {
        let id = Uuid::now_v7();
        for (key, word) in [
            (sea_orm::Value::from(id), id.to_string()),
            (sea_orm::Value::from(5_i64), "5".to_owned()),
            (sea_orm::Value::from(7_i32), "7".to_owned()),
            (sea_orm::Value::from("slug"), "slug".to_owned()),
        ] {
            assert_eq!(LineKey(key).to_string(), word);
        }
    }
}
