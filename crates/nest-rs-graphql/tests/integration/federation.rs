//! `#[entity]` — the operation role a router reaches, and the layers it owes.
//!
//! `_entities` is a `Query`-root field the router calls with **references** —
//! `{__typename, <key fields>}` — for types the client never named. That makes
//! it the one operation whose access posture is invisible from the document a
//! client reads, so what is asserted here is that it is an operation like any
//! other: the guard chain runs on it, its `#[authorize]`/`#[public]` posture
//! runs on it, and the `@key` the router matches on comes from its own
//! arguments.

use async_graphql::{Context, Result, SimpleObject};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, GraphqlGuard, Guard, async_trait, guard};
use nest_rs_http::poem::http::StatusCode;
use nest_rs_testing::TestApp;

/// A schema configured as a subgraph, which every `#[entity]` below needs — the
/// boot refuses one without it.
fn subgraph() -> nest_rs_graphql::GraphqlSetup {
    GraphqlModule::for_root(GraphqlConfig {
        federation: true,
        ..GraphqlConfig::default()
    })
}

/// The federated type. `id` is what the entity resolver takes, so `id` is the
/// `@key` — inferred from the resolver's arguments, never declared twice.
#[derive(SimpleObject)]
struct Widget {
    id: i32,
    label: String,
}

/// Refuses every operation it is bound to, so "the chain runs on `_entities`"
/// is observable rather than assumed.
#[injectable]
#[derive(Default)]
struct Closed;

impl Layer for Closed {}

#[async_trait]
impl Guard for Closed {
    async fn check_graphql(&self, _ctx: &Context<'_>) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("not for this router"))
    }
}

impl GraphqlGuard for Closed {}

#[resolver]
struct WidgetsResolver;

#[operations]
impl WidgetsResolver {
    /// An ordinary query, so the schema has a root field of its own beside the
    /// two the federation spec adds.
    #[query]
    #[public]
    async fn widget(&self, id: i32) -> Widget {
        Widget {
            id,
            label: "queried".into(),
        }
    }

    /// The entity resolver. Its `id` argument is the key the router sends back.
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "by reference".into(),
        })
    }
}

#[module(imports = [GraphqlModule::for_root(GraphqlConfig {
    federation: true,
    disable_introspection: false,
    ..GraphqlConfig::default()
})], providers = [WidgetsResolver])]
struct FederatedModule;

#[resolver]
#[use_guards(Closed)]
struct GatedResolver;

#[operations]
impl GatedResolver {
    #[query]
    #[public]
    async fn ping(&self) -> Result<String> {
        Ok("pong".into())
    }

    /// Same posture as the one above, and a resolver-scope guard on top: what
    /// the router reaches is gated exactly like what a client reaches.
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "never reached".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [GatedResolver, Closed])]
struct GatedModule;

const REFERENCE: &str = r#"
    query($reps: [_Any!]!) {
      _entities(representations: $reps) { ... on Widget { id label } }
    }
"#;

fn representation(id: i32) -> serde_json::Value {
    serde_json::json!({ "__typename": "Widget", "id": id })
}

async fn entities(app: &TestApp, query: &str, id: i32) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": query,
            "variables": { "reps": [representation(id)] },
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    resp.json().await.value().deserialize::<serde_json::Value>()
}

#[tokio::test]
async fn an_entity_resolver_answers_a_reference_the_client_never_named() {
    let app = TestApp::for_module::<FederatedModule>()
        .await
        .expect("a federated schema boots");

    let body = entities(&app, REFERENCE, 7).await;
    let entity = &body["data"]["_entities"][0];
    assert_eq!(
        entity["label"], "by reference",
        "the router's reference reached the entity resolver: {body}",
    );
    assert_eq!(entity["id"], 7, "carrying the key it was resolved by");
}

#[tokio::test]
async fn the_key_a_router_matches_on_is_the_entity_resolvers_own_argument() {
    let app = TestApp::for_module::<FederatedModule>()
        .await
        .expect("a federated schema boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ _service { sdl } }" }))
        .send()
        .await;
    let body = resp.json().await.value().deserialize::<serde_json::Value>();
    let sdl = body["data"]["_service"]["sdl"]
        .as_str()
        .unwrap_or_else(|| panic!("a subgraph publishes its own SDL: {body}"))
        .to_owned();

    assert!(
        sdl.contains(r#"type Widget @key(fields: "id")"#),
        "`@key` is inferred from the entity resolver's arguments, so `#[expose]` \
         never declares a federation key of its own: {sdl}",
    );
    assert!(
        !sdl.contains("_entities(") && !sdl.contains("_service:"),
        "and the exported subgraph SDL strips the two federation fields, as the \
         spec requires of a subgraph schema: {sdl}",
    );
}

#[tokio::test]
async fn the_guard_chain_runs_on_a_reference_the_same_as_on_a_query() {
    let app = TestApp::builder()
        .module::<GatedModule>()
        .use_guards_global([guard::<Closed>()])
        .build()
        .await
        .expect("a gated federated schema boots");

    let body = entities(&app, REFERENCE, 7).await;
    assert!(
        !body.to_string().contains("never reached"),
        "an entity the chain refuses is not resolved by reference either — \
         otherwise every `@key`-ed type is readable from outside every gate in \
         the schema: {body}",
    );
    assert!(
        body["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty()),
        "and it is refused rather than answered empty: {body}",
    );
    assert!(
        body.to_string().contains("not for this router"),
        "with the guard's own denial: {body}",
    );
}

/// An entity resolver *is* the federation surface — async-graphql serves
/// `_service` and `_entities` the moment one exists, whatever the builder was
/// told. So `federation: false` beside an `#[entity]` is a schema that publishes
/// its own SDL while claiming not to be a subgraph, and the boot refuses it
/// rather than letting the flag read as a comment.
#[tokio::test]
async fn an_entity_without_the_subgraph_flag_fails_the_boot() {
    let err = TestApp::for_module::<UnflaggedModule>()
        .await
        .err()
        .expect("declaring an entity while claiming not to be a subgraph is refused");
    let message = err.to_string();

    assert!(
        message.contains("UnflaggedResolver") && message.contains("#[entity]"),
        "the boot error names the resolver that declared it: {message}",
    );
    assert!(
        message.contains("federation"),
        "and the flag that resolves it: {message}",
    );
}

#[resolver]
struct UnflaggedResolver;

#[operations]
impl UnflaggedResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "never mounted".into(),
        })
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [UnflaggedResolver])]
struct UnflaggedModule;

/// Two `#[entity]` resolvers for one type is the duplicate-operation defect in
/// the form the field-name check cannot see: an entity claims a **type**, so
/// there is no clashing SDL field, only a doubled `@key` — and the router
/// reaches whichever linked first, posture included.
#[tokio::test]
async fn two_entity_resolvers_for_one_type_fail_the_boot() {
    let err = TestApp::for_module::<ContestedModule>()
        .await
        .err()
        .expect("two resolvers keying one type is a duplicate, not a merge");
    let message = err.to_string();

    assert!(
        message.contains("entity \"Widget\" keyed by"),
        "the boot error names the contested type and the key shape they contest: {message}",
    );
    assert!(
        message.contains("FirstWidgetResolver") && message.contains("SecondWidgetResolver"),
        "and both claimants, so neither has to be guessed: {message}",
    );
}

#[resolver]
struct FirstWidgetResolver;

#[operations]
impl FirstWidgetResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "first".into(),
        })
    }
}

#[resolver]
struct SecondWidgetResolver;

#[operations]
impl SecondWidgetResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "second".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [FirstWidgetResolver, SecondWidgetResolver])]
struct ContestedModule;

/// The same duplicate written the way it is easiest to write: **one** resolver
/// with two `#[entity]` methods for one type. A check that diffed keys per
/// registration could never see it — one resolver is one registration, so both
/// claims land inside a single pass — and what decided which body the router
/// reached, posture included, was the order the two methods happened to appear
/// in.
#[tokio::test]
async fn two_entity_methods_on_one_resolver_fail_the_boot_too() {
    let err = TestApp::for_module::<SelfContestedModule>()
        .await
        .err()
        .expect("one resolver claiming a key twice is still a duplicate");
    let message = err.to_string();

    assert!(
        message.contains("entity \"Widget\" keyed by \"id\""),
        "naming the type and the shape both methods claim: {message}",
    );
    assert!(
        message.contains("SelfContestedResolver"),
        "and the resolver, which is on both sides of this one: {message}",
    );
}

#[resolver]
struct SelfContestedResolver;

#[operations]
impl SelfContestedResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "first".into(),
        })
    }

    #[entity]
    #[public]
    async fn find_widget_by_id_again(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "second".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [SelfContestedResolver])]
struct SelfContestedModule;

/// The other half, and the one a type-keyed check refused by mistake: Apollo
/// lets a type carry several `@key`s, and async-graphql matches a reference
/// against the **shape**. Two resolvers keying one type by different fields are
/// therefore both reachable, and refusing them would be inventing a rule the
/// spec does not have.
#[tokio::test]
async fn two_keys_of_different_shapes_on_one_type_are_not_a_duplicate() {
    let app = TestApp::for_module::<MultiKeyModule>()
        .await
        .expect("a type carrying two distinct keys is legal federation");

    let by_id = entities(&app, REFERENCE, 4).await;
    assert_eq!(
        by_id["data"]["_entities"][0]["label"], "by id",
        "the `id` key resolves: {by_id}",
    );

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($reps: [_Any!]!) { _entities(representations: $reps) { ... on Widget { label } } }",
            "variables": { "reps": [{ "__typename": "Widget", "slug": "hello" }] },
        }))
        .send()
        .await;
    let by_slug = resp.json().await.value().deserialize::<serde_json::Value>();
    assert_eq!(
        by_slug["data"]["_entities"][0]["label"], "by slug",
        "and so does the second shape, from the other resolver: {by_slug}",
    );
}

#[resolver]
struct WidgetByIdResolver;

#[operations]
impl WidgetByIdResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "by id".into(),
        })
    }
}

#[resolver]
struct WidgetBySlugResolver;

#[operations]
impl WidgetBySlugResolver {
    #[entity]
    #[public]
    async fn find_widget_by_slug(&self, slug: String) -> Result<Widget> {
        Ok(Widget {
            id: slug.len() as i32,
            label: "by slug".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [WidgetByIdResolver, WidgetBySlugResolver])]
struct MultiKeyModule;
