//! HTTP bindings (feature `http`) — the per-route analog of `crate::mcp`.
//!
//! In request order: [`AbilityGuard`](crate::AbilityGuard) builds the request
//! `Ability` from the actor an authn guard attached; `#[authorize(A, S)]` or
//! `#[public(A, S)]` beside a route makes `#[routes]` emit the class gate
//! ([`class_gate`](crate::class_gate), every edge's), run with the route's
//! guards before any interceptor or filter it declares (`403` unless `A` on `S`
//! is granted), and opens the route's reads to that ability; each wire row the
//! handler answers was judged where `#[expose]`'s conversion built it, so the
//! handler checks nothing itself. Who may call is the decorator's to say alone:
//! a route declaring no posture reads withheld, whatever its parameters' types.
//!
//! A route loads its subject by id with `#[authorize(A, bind = S)]`, whose
//! loader lives in `nest-rs-seaorm`.
//!
//! ```
//! # use nest_rs_authz::{AbilityBuilder, Action, Create};
//! # use nest_rs_core::{Layer, async_trait, injectable, input, module};
//! # use nest_rs_guards::{Denial, Guard, GuardFor};
//! # use nest_rs_http::{Json, Result, StatusCode, controller, routes};
//! # use nest_rs_testing::TestApp;
//! # mod users {
//! #     use nest_rs_seaorm::expose;
//! #     use sea_orm::entity::prelude::*;
//! #     #[expose(name = "UserRow")]
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
//! #     #[sea_orm(table_name = "users")]
//! #     pub struct Model {
//! #         #[sea_orm(primary_key)]
//! #         #[expose]
//! #         pub id: i32,
//! #         #[expose]
//! #         pub name: String,
//! #     }
//! #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
//! #     pub enum Relation {}
//! #     impl ActiveModelBehavior for ActiveModel {}
//! # }
//! # #[input]
//! # struct CreateUser {
//! #     name: String,
//! # }
//! # #[input]
//! # struct User {
//! #     id: i32,
//! #     name: String,
//! # }
//! # impl From<CreateUser> for User {
//! #     fn from(input: CreateUser) -> Self {
//! #         User { id: 1, name: input.name }
//! #     }
//! # }
//! # #[injectable]
//! # #[derive(Default, Layer)]
//! # struct Grants;
//! # #[async_trait]
//! # impl Guard for Grants {
//! #     fn __attaches_ability(&self) -> bool {
//! #         true
//! #     }
//! #     fn phase(&self) -> nest_rs_guards::GuardPhase {
//! #         nest_rs_guards::GuardPhase::Authorization
//! #     }
//! #     async fn check(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> std::result::Result<(), Denial> {
//! #         let mut ab = AbilityBuilder::new();
//! #         if nest_rs_http::HttpView::of(cx)?.request().header("x-admin").is_some() {
//! #             ab.can(Action::Manage, users::Entity);
//! #         }
//! #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
//! #         cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
//! #         Ok(())
//! #     }
//! # }
//! # impl GuardFor<nest_rs_http::HttpView> for Grants {}
//! # #[controller(path = "/users")]
//! # #[use_guards(Grants)]
//! # #[derive(Default)]
//! # struct UsersController;
//! # #[routes]
//! # impl UsersController {
//! #[post("/")]
//! #[authorize(Create, users::Entity)]
//! async fn create(&self, Json(input): Json<CreateUser>) -> Result<Json<User>> {
//!     Ok(Json(User::from(input)))
//! }
//! # }
//! # #[module(providers = [Grants, UsersController])]
//! # struct UsersModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//! # let app = TestApp::for_module::<UsersModule>().await?;
//! # let ada = serde_json::json!({ "name": "ada" });
//! # let admin = app.http().post("/users").header("x-admin", "").body_json(&ada).send().await;
//! # let stranger = app.http().post("/users").body_json(&ada).send().await;
//!
//! admin.assert_json(serde_json::json!({ "id": 1, "name": "ada" })).await;
//! stranger.assert_status(StatusCode::FORBIDDEN);
//! # Ok(())
//! # }
//! ```
