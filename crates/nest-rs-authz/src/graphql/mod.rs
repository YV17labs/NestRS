//! GraphQL bindings (feature `graphql`) — the resolver analog of
//! `crate::http`: [`ability`] reads the ability the field's unit carries, which
//! the guards attached — at the `/graphql` request (the app's global pool, or
//! `EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)>` a module declares)
//! or at the field itself.
//!
//! A resolver writes no mask: `#[authorize(Action, Entity)]` on a
//! `#[query]`/`#[mutation]` makes `#[resolver]` emit the class gate
//! ([`class_gate`](crate::class_gate), every edge's), and each wire row the
//! operation answers was judged for its caller where it was built.
//!
//! The row an operation loads by id, `#[authorize(A, bind = S)]`, is
//! `nest-rs-seaorm`'s.
//!
//! ```
//! # use nest_rs_authz::{AbilityBuilder, Action, Read};
//! # use nest_rs_core::{Layer, async_trait, injectable, module};
//! # use nest_rs_graphql::async_graphql::Result;
//! # use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
//! # use nest_rs_guards::{Denial, Guard, GuardFor};
//! # use nest_rs_http::EndpointGuards;
//! # use nest_rs_testing::TestApp;
//! # mod users {
//! #     use nest_rs_seaorm::expose;
//! #     use sea_orm::entity::prelude::*;
//! #     #[expose(name = "User", graphql)]
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
//! #[resolver]
//! struct UsersResolver;
//!
//! #[operations]
//! impl UsersResolver {
//!     #[query]
//!     #[authorize(Read, users::Entity)]
//!     async fn users(&self) -> Result<Vec<User>> {
//! #       let rows = [users::Model { id: 1, name: "ada".into() }];
//!         Ok(rows.iter().map(User::from).collect())
//!     }
//! }
//! # #[module(
//! #     imports = [GraphqlModule],
//! #     providers = [
//! #         PassGuard,
//! #         Grants,
//! #         EndpointGuards<GraphqlView, (PassGuard, Grants)>,
//! #         UsersResolver,
//! #     ],
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

mod context;

pub use context::{ability, forbidden};
