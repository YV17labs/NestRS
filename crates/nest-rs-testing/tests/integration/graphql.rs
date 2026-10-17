//! `GraphqlSocket`: a subscription driven over graphql-transport-ws runs under
//! what a test attaches, as a real socket runs under what its upgrade's guards
//! attached.

use std::sync::Arc;

use nest_rs_core::{AmbientValue, module};
use nest_rs_graphql::async_graphql::Context;
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{GraphqlModule, forward_principal, operations, resolver};
use nest_rs_testing::TestApp;

/// The caller a guard at the upgrade would attach.
#[derive(Clone)]
struct Caller(&'static str);

impl AmbientValue for Caller {}

forward_principal!(Caller);

#[resolver]
struct CallerResolver;

#[operations]
impl CallerResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }

    /// The caller each source holds: the ambient one, then the one the seed
    /// forwarded into the connection's context.
    #[subscription]
    #[public]
    fn caller(&self, ctx: &Context<'_>) -> impl Stream<Item = String> {
        let ambient = nest_rs_core::ambient::<Caller>().map_or("none", |caller| caller.0);
        let forwarded = ctx.data_opt::<Caller>().map_or("none", |caller| caller.0);
        stream::iter([format!("{ambient} {forwarded}")])
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [CallerResolver])]
struct CallerModule;

async fn first_caller(attached: Option<Caller>) -> serde_json::Value {
    let app = TestApp::for_module::<CallerModule>().await.expect("boots");
    let builder = app.graphql_socket();
    let mut socket = match attached {
        Some(caller) => builder.attach(Arc::new(caller)),
        None => builder,
    }
    .open();
    socket.connect().await;
    socket.subscribe("1", "subscription { caller }");
    socket.next_item("1").await.expect("an item")
}

#[tokio::test]
async fn an_attached_value_is_ambient_for_each_operation_and_forwarded_by_the_seeds() {
    let item = first_caller(Some(Caller("ada"))).await;
    assert_eq!(item["data"]["caller"], "ada ada", "{item}");
}

#[tokio::test]
async fn a_socket_attaching_nothing_carries_nothing() {
    let item = first_caller(None).await;
    assert_eq!(item["data"]["caller"], "none none", "{item}");
}
