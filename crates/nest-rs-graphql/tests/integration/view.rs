//! `src/view.rs`: what a guard reads of the field it checks, at every site that
//! dispatches one — a root field of each kind, an entity a router reaches
//! through `_entities`, the two federation fields and a field resolver.

use std::sync::Mutex;

use async_graphql::parser::types::OperationType;
use async_graphql::{Result, SimpleObject};
use nest_rs_core::{Layer, UnitContext, injectable, module};
use nest_rs_graphql::{FieldRole, GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait, guard};
use nest_rs_testing::TestApp;

/// One field as a guard saw it.
type Seen = (&'static str, OperationType, FieldRole);

/// Every field the witness checked, in order. Each test runs in its own process.
static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

/// Admits every field, recording what its view says.
#[injectable]
#[derive(Default)]
struct Witness;

impl Layer for Witness {}

#[async_trait]
impl Guard for Witness {
    async fn check(&self, cx: &mut UnitContext<'_>) -> std::result::Result<(), Denial> {
        let view = cx
            .switch_to::<GraphqlView>()
            .ok_or_else(|| Denial::internal("a field has the GraphQL view"))?;
        let seen = (view.field(), view.operation(), view.role());
        SEEN.lock().expect("only this guard writes it").push(seen);
        Ok(())
    }
}

impl GuardFor<GraphqlView> for Witness {}

#[derive(SimpleObject)]
#[graphql(complex)]
struct Gadget {
    id: i32,
}

#[resolver]
#[use_guards(Witness)]
struct GadgetsResolver;

#[operations]
impl GadgetsResolver {
    #[query]
    #[public]
    async fn gadget(&self, id: i32) -> Result<Gadget> {
        Ok(Gadget { id })
    }

    #[mutation]
    #[public]
    async fn build_gadget(&self, id: i32) -> Result<Gadget> {
        Ok(Gadget { id })
    }

    #[entity]
    #[public]
    async fn find_gadget_by_id(&self, id: i32) -> Result<Gadget> {
        Ok(Gadget { id })
    }

    #[field_resolver]
    async fn label(&self, parent: &Gadget) -> Result<String> {
        Ok(format!("gadget {}", parent.id))
    }
}

#[module(imports = [GraphqlModule::for_root(GraphqlConfig {
    federation: true,
    disable_introspection: false,
    ..GraphqlConfig::default()
})], providers = [GadgetsResolver, Witness])]
struct GadgetsModule;

/// What the witness saw while `query` ran, the query answered cleanly.
async fn seen_under(app: &TestApp, query: serde_json::Value) -> Vec<Seen> {
    SEEN.lock().expect("only the witness writes it").clear();
    let resp = app.http().post("/graphql").body_json(&query).send().await;
    resp.assert_status_is_ok();
    let body = resp.json().await.value().deserialize::<serde_json::Value>();
    assert!(body["errors"].is_null(), "{body}");
    std::mem::take(&mut *SEEN.lock().expect("only the witness writes it"))
}

/// The resolver's own guard, with no app-wide pool: a field resolver runs it
/// too, since no root field ran it for the request.
#[tokio::test]
async fn a_resolvers_guard_reads_each_field_it_checks_and_its_role() {
    let app = TestApp::for_module::<GadgetsModule>()
        .await
        .expect("a federated schema boots");

    assert_eq!(
        seen_under(
            &app,
            serde_json::json!({ "query": "{ gadget(id: 1) { id label } }" })
        )
        .await,
        [
            ("gadget", OperationType::Query, FieldRole::RootField),
            ("label", OperationType::Query, FieldRole::Nested),
        ],
        "a query's root field, then the field resolved under it",
    );
    assert_eq!(
        seen_under(
            &app,
            serde_json::json!({ "query": "mutation { buildGadget(id: 2) { id } }" })
        )
        .await,
        [("buildGadget", OperationType::Mutation, FieldRole::RootField)],
        "a mutation's root field, under the mutation",
    );
    assert_eq!(
        seen_under(&app, entities_of(3)).await,
        [("findGadgetById", OperationType::Query, FieldRole::Entity)],
        "the entity a router's reference reaches",
    );
}

/// The app-wide pool at the two federation fields, which no resolver declares.
#[tokio::test]
async fn the_pool_reads_a_federation_field_as_its_own_role() {
    let app = TestApp::builder()
        .module::<GadgetsModule>()
        .use_guards_global([guard::<Witness>()])
        .build()
        .await
        .expect("a federated schema under a global pool boots");

    assert_eq!(
        seen_under(&app, serde_json::json!({ "query": "{ _service { sdl } }" })).await,
        [("_service", OperationType::Query, FieldRole::Federation)],
        "the subgraph's SDL",
    );
    assert_eq!(
        seen_under(&app, entities_of(3)).await,
        [("_entities", OperationType::Query, FieldRole::Federation)],
        "the router's call, where the pool ran for every entity it reaches",
    );
}

/// The `_entities` call a router makes for one `Gadget` reference.
fn entities_of(id: i32) -> serde_json::Value {
    serde_json::json!({
        "query": "query($reps: [_Any!]!) { _entities(representations: $reps) { ... on Gadget { id } } }",
        "variables": { "reps": [{ "__typename": "Gadget", "id": id }] },
    })
}
