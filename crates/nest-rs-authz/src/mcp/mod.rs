//! MCP bindings (feature `mcp`) — the tool-host analog of `crate::graphql`.
//!
//! The guards at the `/mcp` request — the app's global pool, or
//! `EndpointGuards<McpView, (AuthnGuard, AuthzGuard)>` a module declares —
//! authenticate it with the same chain controllers use, and the caller's
//! [`Ability`](crate::Ability) they attach is ambient for each operation the
//! request carries.
//!
//! A host checks nothing itself: `#[authorize(Action, Subject)]` beside an
//! operation of `#[tools]` makes it emit the class gate
//! ([`class_gate`](crate::class_gate), every edge's), exactly as `#[operations]`
//! does on GraphQL and `#[routes]` on HTTP, and each wire row the operation
//! answers was judged for its caller where it was built.
//!
//! ```
//! # use nest_rs_authz::{AbilityBuilder, Action, Read};
//! # use nest_rs_core::{Layer, async_trait, injectable, module};
//! # use nest_rs_guards::{Denial, Guard, GuardFor};
//! # use nest_rs_http::EndpointGuards;
//! # use nest_rs_mcp::{Json, McpError, McpView, mcp, tools};
//! # use nest_rs_testing::TestApp;
//! # use nest_rs_testing::mcp::call_tool_as;
//! # mod users {
//! #     use nest_rs_seaorm::expose;
//! #     use sea_orm::entity::prelude::*;
//! #     #[expose(name = "User")]
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
//! #     #[sea_orm(table_name = "users")]
//! #     pub struct Model {
//! #         #[sea_orm(primary_key)]
//! #         #[expose]
//! #         pub id: i32,
//! #         #[expose(nullable)]
//! #         pub name: String,
//! #     }
//! #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
//! #     pub enum Relation {}
//! #     impl ActiveModelBehavior for ActiveModel {}
//! # }
//! # use users::User;
//! # #[injectable]
//! # #[derive(Default, Layer)]
//! # struct PassGuard;
//! # #[async_trait]
//! # impl Guard for PassGuard {
//! #     async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
//! #         nest_rs_core::__private::set_actor_id("caller");
//! #         Ok(())
//! #     }
//! #     async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
//! #         Ok(())
//! #     }
//! # }
//! # impl GuardFor<nest_rs_http::HttpView> for PassGuard {}
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
//! #     async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
//! #         let mut ab = AbilityBuilder::new();
//! #         if nest_rs_http::HttpView::of(cx)?.request().header("x-viewer").is_some() {
//! #             ab.can(Action::Read, users::Entity).fields([users::Column::Id]);
//! #         }
//! #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
//! #         cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
//! #         Ok(())
//! #     }
//! #     async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
//! #         Ok(())
//! #     }
//! # }
//! # impl GuardFor<nest_rs_http::HttpView> for Grants {}
//! # #[mcp(path = "/mcp")]
//! # #[derive(Clone, Default)]
//! # struct UsersTool;
//! #[tools]
//! impl UsersTool {
//!     /// List the people the caller may see.
//!     #[tool]
//!     #[authorize(Read, users::Entity)]
//!     async fn list_people(&self) -> Result<Json<Vec<User>>, McpError> {
//! #       let rows = [users::Model { id: 1, name: "ada".into() }];
//!         Ok(Json(rows.iter().map(User::from).collect()))
//!     }
//! }
//! # #[module(providers = [
//! #     PassGuard,
//! #     Grants,
//! #     EndpointGuards<McpView, (PassGuard, Grants)>,
//! #     UsersTool,
//! # ])]
//! # struct UsersModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let app = TestApp::for_module::<UsersModule>().await?;
//! # let args = serde_json::json!({});
//!
//! let viewer = call_tool_as(app.http(), "/mcp", "list_people", None, &[("x-viewer", "")], args.clone()).await;
//! assert!(viewer.contains(r#""id":1"#) && !viewer.contains("ada"));
//!
//! let stranger = call_tool_as(app.http(), "/mcp", "list_people", None, &[], args).await;
//! assert!(stranger.contains("forbidden"));
//! # Ok(())
//! # }
//! ```
//!
//! These read the *ambient* ability: rmcp dispatches an operation on its own
//! task with no context value, so the endpoint re-installs what the request's
//! guards attached around each operation.
