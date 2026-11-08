//! WebSocket bindings (feature `ws`) — the per-message analog of `crate::mcp`.
//!
//! A gateway writes no mask: `#[authorize(Action, Entity)]` beside a
//! `#[subscribe_message]` makes `#[messages]` emit the class gate
//! ([`class_gate`](crate::class_gate), every edge's), run in the message's guard
//! stage, exactly as `#[tools]` does on MCP, `#[operations]` on GraphQL and
//! `#[routes]` on HTTP, and each wire row its reply carries was judged for its
//! caller where it was built. `Gateway::dispatch`, below, calls the handler on
//! its own, behind the class gate.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_authz::{AbilityBuilder, Action, Read};
//! # use nest_rs_authz::__private::install_ability;
//! # use nest_rs_ws::{Gateway, WsClient, WsReply, gateway, messages};
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
//! #     install_ability(Arc::new(ability), gateway.dispatch(&client, "users.list", serde_json::Value::Null))
//! # };
//!
//! let mut ids_only = AbilityBuilder::new();
//! ids_only.can(Action::Read, users::Entity).fields([users::Column::Id]);
//! let WsReply::Reply(judged) = list(ids_only.build()?).await else { panic!() };
//! assert_eq!(judged, serde_json::json!([{ "id": 1, "name": null }]));
//!
//! let refused = list(AbilityBuilder::new().build()?).await;
//! assert!(matches!(refused, WsReply::Error(_)));
//! # Ok(())
//! # }
//! ```
//!
//! A gateway is `Gated`, so the HTTP guards run on the upgrade, and the
//! ability they attach crosses the connection into each message.
