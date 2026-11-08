//! [`Authorize<A, S>`] — the route-level class gate for action `A` on
//! subject `S`, and the type that opens the route's reads.

use std::marker::PhantomData;

use nest_rs_http::{Request, ResponseShaping, RouteResponseShaper};

use crate::{ActionMarker, Subject};

/// Enforcement plumbing for action `A` on subject `S`. A route whose posture
/// names it, through `#[authorize(A, S)]` or `#[public(A, S)]`, runs
/// [`class_gate`](crate::class_gate) with its guards, before any interceptor or
/// filter the route declares, and reads under the ability they attached when `S`
/// is an entity. Class-level only — the per-row filter and the judgement each
/// wire row passes where it is built enforce conditions.
///
/// # Don't write this — write `#[authorize(Action, Entity)]`
///
/// The posture of an HTTP route is declared by the decorator, exactly as on a
/// `#[query]`/`#[mutation]`:
///
/// ```
/// # use nest_rs_authz::{AbilityBuilder, Action, Create};
/// # use nest_rs_core::{Layer, async_trait, injectable, input, module};
/// # use nest_rs_guards::{Denial, Guard, GuardFor};
/// # use nest_rs_http::{Json, Result, StatusCode, controller, routes};
/// # use nest_rs_testing::TestApp;
/// # mod users {
/// #     use nest_rs_seaorm::expose;
/// #     use sea_orm::entity::prelude::*;
/// #     #[expose(name = "UserRow")]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "users")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         #[expose]
/// #         pub id: i32,
/// #         #[expose]
/// #         pub name: String,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # #[input]
/// # struct CreateUser {
/// #     name: String,
/// # }
/// # #[input]
/// # struct User {
/// #     id: i32,
/// #     name: String,
/// # }
/// # impl From<CreateUser> for User {
/// #     fn from(input: CreateUser) -> Self {
/// #         User { id: 1, name: input.name }
/// #     }
/// # }
/// # #[injectable]
/// # #[derive(Default, Layer)]
/// # struct Grants;
/// # #[async_trait]
/// # impl Guard for Grants {
/// #     fn __attaches_ability(&self) -> bool {
/// #         true
/// #     }
/// #     fn phase(&self) -> nest_rs_guards::GuardPhase {
/// #         nest_rs_guards::GuardPhase::Authorization
/// #     }
/// #     async fn check(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> std::result::Result<(), Denial> {
/// #         let mut ab = AbilityBuilder::new();
/// #         if nest_rs_http::HttpView::of(cx)?.request().header("x-admin").is_some() {
/// #             ab.can(Action::Manage, users::Entity);
/// #         }
/// #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
/// #         cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
/// #         Ok(())
/// #     }
/// # }
/// # impl GuardFor<nest_rs_http::HttpView> for Grants {}
/// # #[controller(path = "/users")]
/// # #[use_guards(Grants)]
/// # #[derive(Default)]
/// # struct UsersController;
/// # #[routes]
/// # impl UsersController {
/// #[post("/")]
/// #[authorize(Create, users::Entity)]
/// async fn create(&self, Json(input): Json<CreateUser>) -> Result<Json<User>> {
///     Ok(Json(User::from(input)))
/// }
/// # }
/// # #[module(providers = [Grants, UsersController])]
/// # struct UsersModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
/// # let app = TestApp::for_module::<UsersModule>().await?;
/// # let ada = serde_json::json!({ "name": "ada" });
/// # let admin = app.http().post("/users").header("x-admin", "").body_json(&ada).send().await;
/// # let stranger = app.http().post("/users").body_json(&ada).send().await;
///
/// admin.assert_json(serde_json::json!({ "id": 1, "name": "ada" })).await;
/// stranger.assert_status(StatusCode::FORBIDDEN);
/// # Ok(())
/// # }
/// ```
///
/// `#[routes]` reads that as this type without a parameter, and
/// `#[public(Action, Entity)]`, on a route a caller with no credential reaches, as
/// the same type. Never a handler parameter: the posture is a decorator.
///
/// ```compile_fail,E0277
/// # use nest_rs_authz::Read;
/// # use nest_rs_authz::http::Authorize;
/// # use nest_rs_http::{Result, controller, routes};
/// # mod posts {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "posts")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # #[controller(path = "/posts")]
/// # struct PostsController;
/// # #[routes]
/// # impl PostsController {
/// #[get("/")]
/// #[public]
/// async fn list(&self, _authz: Authorize<Read, posts::Entity>) -> Result<String> {
///     Ok(String::new())
/// }
/// # }
/// # fn main() {}
/// ```
pub struct Authorize<A, S>(PhantomData<fn() -> (A, S)>);

/// The class gate a posture declares, run with the route's guards. An entity's
/// opens the route's reads to the ability they attached; a subject no table holds
/// opens none, so the route reads as one no posture names. Nothing reshapes the
/// reply: each wire row it carries was judged where it was built.
impl<A, S> RouteResponseShaper for Authorize<A, S>
where
    A: ActionMarker,
    S: Subject,
{
    const GATE: Option<nest_rs_http::__private::GateFn> = Some(crate::class_gate::<A, S>);
    const SHAPES: bool = S::__ROWS;

    fn capture(_req: &Request) -> Option<Box<dyn ResponseShaping>> {
        None
    }
}
