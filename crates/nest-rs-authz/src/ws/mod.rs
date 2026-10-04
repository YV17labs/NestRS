//! WebSocket bindings (feature `ws`) — the per-message analog of [`crate::mcp`].
//!
//! [`authorize`] is the class-level gate and [`masked_reply_for`] masks a
//! message's reply. A gateway calls neither directly:
//! `#[authorize(Action, Entity)]` beside a `#[subscribe_message]` makes
//! `#[messages]` emit both, exactly as `#[tools]` does on MCP, `#[operations]` on
//! GraphQL and `#[routes]` on HTTP.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_authz::{AbilityBuilder, Action, Read, with_ability};
//! # use nest_rs_ws::{Gateway, WsClient, WsReply, gateway, messages};
//! # mod users {
//! #     use nest_rs_resource::expose;
//! #     use sea_orm::entity::prelude::*;
//! #     #[expose(name = "User")]
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
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
//! # use users::User;
//! # #[derive(Debug, thiserror::Error)]
//! # #[error("the users could not be listed")]
//! # struct ServiceError;
//! # struct UsersService;
//! # impl UsersService {
//! #     async fn list(&self) -> Result<Vec<users::Model>, ServiceError> {
//! #         Ok(vec![users::Model { id: 1, name: "ada".into() }])
//! #     }
//! # }
//! # #[gateway(path = "/ws/users")]
//! # struct UsersGateway {
//! #     #[inject]
//! #     svc: Arc<UsersService>,
//! # }
//! #[messages]
//! impl UsersGateway {
//!     #[subscribe_message("users.list")]
//!     #[authorize(Read, users::Entity)]
//!     async fn list(&self) -> Result<Vec<User>, ServiceError> {
//!         Ok(self.svc.list().await?.iter().map(User::from).collect())
//!     }
//! }
//! # #[nest_rs_core::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let gateway = UsersGateway { svc: Arc::new(UsersService) };
//! # let client = WsClient::for_test();
//! # let list = |ability| {
//! #     with_ability(Arc::new(ability), gateway.dispatch(&client, "users.list", serde_json::Value::Null))
//! # };
//!
//! let mut ids_only = AbilityBuilder::new();
//! ids_only.can(Action::Read, users::Entity).fields([users::Column::Id]);
//! let WsReply::Reply(masked) = list(ids_only.build()?).await else { panic!() };
//! assert_eq!(masked, serde_json::json!([{ "id": 1 }]));
//!
//! let refused = list(AbilityBuilder::new().build()?).await;
//! assert!(matches!(refused, WsReply::Error(_)));
//! # Ok(())
//! # }
//! ```
//!
//! # Why there is no `WsAbilityBridge`
//!
//! GraphQL and MCP are `EdgePosture::Exempt`, so each needs a bridge to re-run
//! the guard chain *in band* per operation. A WS gateway is `Guarded`: the upgrade
//! is an HTTP `GET`, so the real HTTP guards run on it at the edge and there is no
//! chain to re-run. What the connection *cannot* keep is its task-locals — those
//! unwound when the upgrade returned — so the only thing to re-establish per
//! message is the ambient state, and that is `nest_rs_seaorm::ws::WsDataContext`
//! (`dyn SocketContext`), not a guard.
//!
//! Hence one asymmetry worth stating plainly: on GraphQL and MCP the *guard*
//! installs the ability, on WS the *data context* does. The gate below reads it
//! the same way either way — [`current_ability`](crate::current_ability) — which
//! is what lets one `#[authorize]` mean one thing on all four transports.

mod authorize;
mod mask;

pub use authorize::authorize;
pub use mask::masked_reply_for;
