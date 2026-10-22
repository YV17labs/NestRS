//! MCP bindings (feature `mcp`) — the tool-host analog of [`crate::graphql`].
//!
//! The guards at the `/mcp` request — the app's global pool, or
//! `EndpointGuards<McpView, (AuthnGuard, AuthzGuard)>` a module declares —
//! authenticate it with the same chain controllers use, and the caller's
//! [`Ability`](crate::Ability) they attach is ambient for each operation the
//! request carries. [`masked_value_for`] masks an operation's return value.
//!
//! A host does not call it directly: `#[authorize(Action, Entity)]` beside a
//! `#[tool]` / `#[prompt]` makes `#[tools]` emit the class gate
//! ([`class_gate`](crate::class_gate), every edge's) and the mask, exactly as
//! `#[operations]` does on GraphQL and `#[routes]` on HTTP.
//!
//! ```
//! # use nest_rs_authz::{AbilityBuilder, Action, Read};
//! # use nest_rs_core::{Layer, injectable, input, module};
//! # use nest_rs_guards::{Denial, Guard, GuardFor};
//! # use nest_rs_http::{EndpointGuards, async_trait};
//! # use nest_rs_mcp::{Json, McpError, McpView, mcp, tools};
//! # use nest_rs_testing::TestApp;
//! # use nest_rs_testing::mcp::call_tool_as;
//! # mod users {
//! #     use sea_orm::entity::prelude::*;
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
//! #     #[sea_orm(table_name = "users")]
//! #     pub struct Model {
//! #         #[sea_orm(primary_key)]
//! #         pub id: i32,
//! #         pub name: String,
//! #     }
//! #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
//! #     pub enum Relation {}
//! #     impl ActiveModelBehavior for ActiveModel {}
//! # }
//! # impl nest_rs_authz::WireModelDefaults for users::Entity {}
//! # #[input]
//! # struct User {
//! #     id: i32,
//! #     name: Option<String>,
//! # }
//! # #[injectable]
//! # #[derive(Default)]
//! # struct PassGuard;
//! # impl Layer for PassGuard {}
//! # #[async_trait]
//! # impl Guard for PassGuard {
//! #     async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
//! #         nest_rs_core::__private::set_actor_id("caller");
//! #         Ok(())
//! #     }
//! # }
//! # impl GuardFor<nest_rs_http::HttpView> for PassGuard {}
//! # #[injectable]
//! # #[derive(Default)]
//! # struct Grants;
//! # impl Layer for Grants {}
//! # #[async_trait]
//! # impl Guard for Grants {
//! #     async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
//! #         let mut ab = AbilityBuilder::new();
//! #         if nest_rs_http::HttpView::of(cx)?.request().header("x-viewer").is_some() {
//! #             ab.can(Action::Read, users::Entity).fields([users::Column::Id]);
//! #         }
//! #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
//! #         cx.attach(ability);
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
//!         // gate + response masking are emitted by the macro
//! #       Ok(Json(vec![User { id: 1, name: Some("ada".into()) }]))
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

mod mask;

pub use mask::masked_value_for;
