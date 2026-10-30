//! `GraphqlSocket`: a subscription driven over graphql-transport-ws runs under
//! what a test attaches, as a real socket runs under what its upgrade's guards
//! attached.

use std::sync::Arc;

use nest_rs_core::{AmbientValue, Ctx, module};
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;

/// The caller a guard at the upgrade would attach.
#[derive(Clone)]
struct Caller(&'static str);

impl AmbientValue for Caller {}

#[resolver]
struct CallerResolver;

#[operations]
impl CallerResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }

    /// The caller the operation's ambient context holds, read as its
    /// parameter.
    #[subscription]
    #[public]
    fn caller(&self, caller: Ctx<Caller>) -> impl Stream<Item = String> {
        stream::iter([caller.into_inner().0.to_owned()])
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
async fn an_attached_value_reaches_each_operation_as_its_parameter() {
    let item = first_caller(Some(Caller("ada"))).await;
    assert_eq!(item["data"]["caller"], "ada", "{item}");
}

#[tokio::test]
async fn a_socket_attaching_nothing_answers_the_operation_opaquely() {
    let item = first_caller(None).await;
    assert_eq!(item["data"], serde_json::Value::Null, "{item}");
    assert_eq!(
        item["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{item}"
    );
    assert_eq!(
        item["errors"][0]["extensions"]["code"], "INTERNAL",
        "{item}"
    );
    let said = item.to_string();
    assert!(
        !said.contains("Caller"),
        "the answer names the type: {said}"
    );
}
