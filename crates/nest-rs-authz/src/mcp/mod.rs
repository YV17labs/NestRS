//! MCP bindings (feature `mcp`) — the tool-host analog of [`crate::graphql`].
//!
//! [`McpAbilityBridge`] is the endpoint's per-operation guard: it authenticates
//! each `/mcp` request with the same chain controllers use and installs the
//! caller's ambient [`Ability`](crate::Ability) for the operation's duration.
//! [`authorize`](fn@authorize) is the class-level gate and [`masked_value_for`] masks an
//! operation's return value.
//!
//! A host does not call the last two directly: `#[authorize(Action, Entity)]`
//! beside a `#[tool]` / `#[prompt]` makes `#[tools]` emit both, exactly as
//! `#[operations]` does on GraphQL and `#[routes]` on HTTP.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_authz::mcp::McpAbilityBridge;
//! # use nest_rs_authz::{AbilityBuilder, Action, Read};
//! # use nest_rs_core::{Layer, injectable, input, module};
//! # use nest_rs_guards::{Denial, Guard, HttpGuard};
//! # use nest_rs_http::async_trait;
//! # use nest_rs_http::poem::Request;
//! # use nest_rs_mcp::{Json, McpError, McpOperationGuard, mcp, tools};
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
//! # impl nest_rs_resource::WireModelDefaults for users::Entity {}
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
//! # impl Guard for PassGuard {}
//! # #[injectable]
//! # #[derive(Default)]
//! # struct Grants;
//! # impl Layer for Grants {}
//! # #[async_trait]
//! # impl Guard for Grants {
//! #     async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
//! #         let mut ab = AbilityBuilder::new();
//! #         if req.headers().contains_key("x-viewer") {
//! #             ab.can(Action::Read, users::Entity).fields([users::Column::Id]);
//! #         }
//! #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
//! #         req.extensions_mut().insert(Arc::new(ability));
//! #         Ok(())
//! #     }
//! # }
//! # impl HttpGuard for Grants {}
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
//! # type Bridge = McpAbilityBridge<PassGuard, Grants>;
//! # #[module(providers = [PassGuard, Grants, Bridge as dyn McpOperationGuard, UsersTool])]
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
//! task with no context value, so the operation guard installs it in its `around`.
//!
//! Data-coupled bindings live in `nest_rs_seaorm::mcp` (`McpDataContext`).

mod authorize;
mod bridge;
mod mask;

pub use authorize::authorize;
pub use bridge::McpAbilityBridge;
pub use mask::masked_value_for;
