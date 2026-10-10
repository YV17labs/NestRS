//! GraphQL bindings (feature `graphql`) — the resolver analog of
//! [`crate::http`]: [`GraphqlAbilityBridge`] is the per-operation guard that
//! authenticates and installs the ambient ability; [`authorize`](fn@authorize) is the
//! class-level gate; [`masked_value_for`] masks a resolver's return value;
//! [`ability`] accesses the per-request ability. Importing this module submits
//! the `GraphqlContextSeed` that forwards `Arc<Ability>` into each operation's
//! GraphQL context.
//!
//! A resolver does not call these directly: `#[authorize(Action, Entity)]` on
//! a `#[query]`/`#[mutation]` makes `#[resolver]` emit both the gate and the
//! response mask — the GraphQL analog of the HTTP `Authorize<A, E>` extractor.
//!
//! Data-coupled bindings live in `nest_rs_seaorm::graphql` (`bind`,
//! `LoaderScope`).
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_authz::graphql::GraphqlAbilityBridge;
//! # use nest_rs_authz::{AbilityBuilder, Action, Read};
//! # use nest_rs_core::{Layer, injectable, module};
//! # use nest_rs_graphql::async_graphql::{Result, SimpleObject};
//! # use nest_rs_graphql::{GraphqlModule, GraphqlOperationGuard, operations, resolver};
//! # use nest_rs_guards::{Denial, Guard, HttpGuard};
//! # use nest_rs_http::async_trait;
//! # use nest_rs_http::poem::Request;
//! # use nest_rs_testing::TestApp;
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
//! # #[derive(SimpleObject, serde::Serialize, serde::Deserialize)]
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
//! # impl HttpGuard for PassGuard {}
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
//! #[resolver]
//! struct UsersResolver;
//!
//! #[operations]
//! impl UsersResolver {
//!     #[query]
//!     #[authorize(Read, users::Entity)]
//!     async fn users(&self) -> Result<Vec<User>> {
//!         // gate + response masking are emitted by the macro
//! #       Ok(vec![User { id: 1, name: Some("ada".into()) }])
//!     }
//! }
//! # type Bridge = GraphqlAbilityBridge<PassGuard, Grants>;
//! # #[module(
//! #     imports = [GraphqlModule::for_root(None)],
//! #     providers = [PassGuard, Grants, Bridge as dyn GraphqlOperationGuard, UsersResolver],
//! # )]
//! # struct UsersModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//! # let app = TestApp::for_module::<UsersModule>().await?;
//! # let query = serde_json::json!({ "query": "{ users { id name } }" });
//! # let viewer = app.http().post("/graphql").header("x-viewer", "").body_json(&query);
//! # let viewer = serde_json::to_value(viewer.send().await.json().await)?;
//! # let stranger = app.http().post("/graphql").body_json(&query);
//! # let stranger = serde_json::to_value(stranger.send().await.json().await)?;
//!
//! assert_eq!(viewer["data"]["users"], serde_json::json!([{ "id": 1, "name": null }]));
//! assert_eq!(stranger["errors"][0]["extensions"]["code"], "FORBIDDEN");
//! # Ok(())
//! # }
//! ```

mod authorize;
mod bridge;
mod context;
mod mask;

pub use authorize::authorize;
pub use bridge::GraphqlAbilityBridge;
pub use context::{ability, forbidden};
pub use mask::{masked_item_for, masked_value_for};
