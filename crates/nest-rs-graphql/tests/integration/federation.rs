//! `#[entity]` — the operation role a router reaches, and the layers it owes:
//! the guard chain and posture run on `_entities`, and the `@key` comes from the
//! resolver's own arguments.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_graphql::{Result, SimpleObject};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::{
    GraphqlConfig, GraphqlModule, GraphqlOperationContext, operations, resolver,
};
use nest_rs_guards::{Denial, GraphqlGuard, Guard, GuardSpecs, async_trait, guard};
use nest_rs_http::poem::http::StatusCode;
use nest_rs_testing::TestApp;

/// A schema configured as a subgraph, which every `#[entity]` below needs.
fn subgraph() -> nest_rs_graphql::GraphqlSetup {
    capped(None)
}

/// The federated type; `id`, the entity resolver's argument, is its `@key`.
#[derive(SimpleObject)]
struct Widget {
    id: i32,
    label: String,
}

/// Refuses every operation it is bound to.
#[injectable]
#[derive(Default)]
struct Closed;

impl Layer for Closed {}

#[async_trait]
impl Guard for Closed {
    async fn check_graphql(
        &self,
        _op: &GraphqlOperationContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("not for this router"))
    }
}

impl GraphqlGuard for Closed {}

#[resolver]
struct WidgetsResolver;

#[operations]
impl WidgetsResolver {
    /// An ordinary query beside the two federation fields.
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

/// A second federated type, resolved by a synchronous entity method.
#[derive(SimpleObject)]
struct Sprocket {
    id: i32,
}

#[resolver]
struct SprocketsResolver;

#[operations]
impl SprocketsResolver {
    /// `fn`, not `async fn`, and a bare return: the emitted entity resolver is
    /// `async` and answers a `Result`.
    #[entity]
    #[public]
    fn find_sprocket_by_id(&self, id: i32) -> Sprocket {
        Sprocket { id }
    }
}

#[module(imports = [GraphqlModule::for_root(GraphqlConfig {
    federation: true,
    disable_introspection: false,
    ..GraphqlConfig::default()
})], providers = [WidgetsResolver, SprocketsResolver])]
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

    /// Same posture, plus a resolver-scope guard.
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

/// POST one GraphQL body and read the response as plain JSON.
async fn post_graphql(app: &TestApp, body: serde_json::Value) -> serde_json::Value {
    let resp = app.http().post("/graphql").body_json(&body).send().await;
    resp.assert_status(StatusCode::OK);
    resp.json().await.value().deserialize::<serde_json::Value>()
}

/// The `_entities` call every test below makes, over the references it is given.
async fn entities_for(app: &TestApp, reps: Vec<serde_json::Value>) -> serde_json::Value {
    post_graphql(
        app,
        serde_json::json!({ "query": REFERENCE, "variables": { "reps": reps } }),
    )
    .await
}

/// One reference, by id.
async fn entities(app: &TestApp, id: i32) -> serde_json::Value {
    entities_for(app, vec![representation(id)]).await
}

/// The subgraph SDL `_service` publishes.
async fn service_sdl(app: &TestApp) -> String {
    let body = post_graphql(app, serde_json::json!({ "query": "{ _service { sdl } }" })).await;
    body["data"]["_service"]["sdl"]
        .as_str()
        .unwrap_or_else(|| panic!("a subgraph publishes its own SDL: {body}"))
        .to_owned()
}

#[tokio::test]
async fn an_entity_resolver_answers_a_reference_the_client_never_named() {
    let app = TestApp::for_module::<FederatedModule>()
        .await
        .expect("a federated schema boots");

    let body = entities(&app, 7).await;
    assert!(
        body["errors"].is_null(),
        "the reference resolves cleanly — a `data` that happens to be right \
         beside an error is a half-answer, and the router reads both: {body}",
    );
    let entity = &body["data"]["_entities"][0];
    assert_eq!(
        entity["label"], "by reference",
        "the router's reference reached the entity resolver: {body}",
    );
    assert_eq!(entity["id"], 7, "carrying the key it was resolved by");
}

#[tokio::test]
async fn a_synchronous_entity_resolver_answers_a_reference() {
    let app = TestApp::for_module::<FederatedModule>()
        .await
        .expect("a federated schema boots");

    let body = post_graphql(
        &app,
        serde_json::json!({
            "query": "query($reps: [_Any!]!) { _entities(representations: $reps) { ... on Sprocket { id } } }",
            "variables": { "reps": [{ "__typename": "Sprocket", "id": 4 }] },
        }),
    )
    .await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["_entities"][0]["id"], 4, "{body}");
}

#[tokio::test]
async fn the_key_a_router_matches_on_is_the_entity_resolvers_own_argument() {
    let app = TestApp::for_module::<FederatedModule>()
        .await
        .expect("a federated schema boots");

    let sdl = service_sdl(&app).await;

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

/// The guard is declared **only** at resolver scope, with no global pool.
#[tokio::test]
async fn a_resolver_scope_guard_runs_on_a_reference_the_same_as_on_a_query() {
    let app = TestApp::for_module::<GatedModule>()
        .await
        .expect("a gated federated schema boots");

    let body = entities(&app, 7).await;
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

/// The guard is declared **only** in the app-wide pool: `_entities` is resolved
/// above the merged root, so the pool reaches it through the schema extension.
#[tokio::test]
async fn a_pooled_guard_refuses_a_reference_before_any_member_answers() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::builder()
        .module::<PooledModule>()
        .use_guards_global([guard::<Closed>()])
        .build()
        .await
        .expect("a federated schema under a global pool boots");

    let body = entities(&app, 7).await;
    assert!(
        body.to_string().contains("not for this router"),
        "the pooled guard's own denial answers the reference: {body}",
    );
    assert_eq!(
        UNGUARDED_ENTITY_RUNS.load(Ordering::SeqCst),
        0,
        "and it refuses *before* a member answers — the body that would have \
         resolved the reference never ran: {body}",
    );

    // A federation field belongs to no provider, so this site carries its own
    // event with a `route` label.
    let event = logs.expect_one("nest_rs::layers", "guard denied the federation field");
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("route").as_deref(),
        Some("POST /graphql (federation)"),
    );
    assert_eq!(event.field("field").as_deref(), Some("_entities"));
    assert!(
        event.field("guard").is_some_and(|g| g.contains("Closed")),
        "the event names the guard that refused, got {:?}",
        event.fields,
    );
}

/// `_service` publishes the whole SDL and is not covered by
/// `disable_introspection`.
#[tokio::test]
async fn a_pooled_guard_refuses_the_subgraph_sdl() {
    let app = TestApp::builder()
        .module::<PooledModule>()
        .use_guards_global([guard::<Closed>()])
        .build()
        .await
        .expect("a federated schema under a global pool boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ _service { sdl } }" }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert!(
        body["data"]["_service"].is_null(),
        "a refused `_service` publishes no SDL: {body}",
    );
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "and it is refused as a native GraphQL error, not a bare HTTP status: {body}",
    );
    assert!(
        body.to_string().contains("not for this router"),
        "carrying the guard's own denial: {body}",
    );
}

/// A pooled guard runs **once per operation**, however many references
/// `_entities` carries.
#[tokio::test]
async fn a_pooled_guard_checks_an_entities_operation_exactly_once() {
    let app = TestApp::builder()
        .module::<CountedModule>()
        .use_guards_global([guard::<Counting>()])
        .build()
        .await
        .expect("a federated schema under a counting pool boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": REFERENCE,
            "variables": { "reps": [representation(1), representation(2), representation(3)] },
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert_eq!(
        body["data"]["_entities"].as_array().map(Vec::len),
        Some(3),
        "the three references resolve: {body}",
    );
    assert_eq!(
        COUNTED_CHECKS.load(Ordering::SeqCst),
        1,
        "and the pooled guard ran once for the operation, not once per \
         representation: {body}",
    );
}

/// The entity body of [`PooledResolver`], counted.
static UNGUARDED_ENTITY_RUNS: AtomicUsize = AtomicUsize::new(0);

#[resolver]
struct PooledResolver;

#[operations]
impl PooledResolver {
    /// An ordinary operation beside the entity.
    #[query]
    #[public]
    async fn widget(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "queried".into(),
        })
    }

    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        UNGUARDED_ENTITY_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(Widget {
            id,
            label: "never reached".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [PooledResolver, Closed])]
struct PooledModule;

/// How many operations [`Counting`] was asked about.
static COUNTED_CHECKS: AtomicUsize = AtomicUsize::new(0);

/// Admits everything and counts.
#[injectable]
#[derive(Default)]
struct Counting;

impl Layer for Counting {}

#[async_trait]
impl Guard for Counting {
    async fn check_graphql(
        &self,
        _op: &GraphqlOperationContext<'_>,
    ) -> std::result::Result<(), Denial> {
        COUNTED_CHECKS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl GraphqlGuard for Counting {}

#[resolver]
struct CountedResolver;

#[operations]
impl CountedResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "counted".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [CountedResolver, Counting])]
struct CountedModule;

/// async-graphql serves `_service` and `_entities` the moment an entity
/// resolver exists, so `federation: false` beside one fails the boot.
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

/// Two `#[entity]` resolvers for one type: no clashing SDL field, only a
/// doubled `@key`.
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

/// The same duplicate on **one** resolver: both claims land in a single pass.
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

/// Apollo lets a type carry several `@key`s: disjoint shapes are both reachable.
#[tokio::test]
async fn two_keys_of_different_shapes_on_one_type_are_not_a_duplicate() {
    let app = TestApp::for_module::<MultiKeyModule>()
        .await
        .expect("a type carrying two distinct keys is legal federation");

    let by_id = entities(&app, 4).await;
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

/// Two `@key` shapes that **overlap** — one a subset of the other — are
/// refused: a reference carrying both satisfies both matchers.
#[tokio::test]
async fn two_resolvers_whose_key_shapes_overlap_fail_the_boot() {
    let err = TestApp::for_module::<OverlappingKeysModule>()
        .await
        .err()
        .expect("two resolvers whose key shapes overlap cannot both be reachable");
    let message = err.to_string();

    assert!(
        message.contains(r#"entity "Gizmo" keyed by "id""#)
            && message.contains(r#"by "id tenant""#),
        "the boot error names the contested type and both key shapes: {message}",
    );
    assert!(
        message.contains("GizmoBroadResolver") && message.contains("GizmoNarrowResolver"),
        "and both claimants, so neither has to be guessed: {message}",
    );
}

/// The federated type the overlap cases key two ways.
#[derive(SimpleObject)]
struct Gizmo {
    id: i32,
    tenant: String,
    label: String,
}

#[resolver]
struct GizmoBroadResolver;

#[operations]
impl GizmoBroadResolver {
    #[entity]
    #[public]
    async fn find_gizmo_by_id(&self, id: i32) -> Result<Gizmo> {
        Ok(Gizmo {
            id,
            tenant: "any".into(),
            label: "by id".into(),
        })
    }
}

#[resolver]
#[use_guards(Closed)]
struct GizmoNarrowResolver;

#[operations]
impl GizmoNarrowResolver {
    #[entity]
    #[public]
    async fn find_gizmo_by_id_and_tenant(&self, id: i32, tenant: String) -> Result<Gizmo> {
        Ok(Gizmo {
            id,
            tenant,
            label: "by id and tenant".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [GizmoBroadResolver, GizmoNarrowResolver, Closed])]
struct OverlappingKeysModule;

/// The same two shapes on **one** resolver: async-graphql sorts its matchers by
/// `args.len()`, not key arity, so the rule is the same at both scopes.
#[tokio::test]
async fn two_overlapping_keys_on_one_resolver_fail_the_boot_too() {
    let err = TestApp::for_module::<OneResolverTwoKeysModule>()
        .await
        .err()
        .expect("one resolver may not claim two overlapping shapes either");
    let message = err.to_string();

    assert!(
        message.contains(r#"entity "Gadget" keyed by "id""#)
            && message.contains(r#"by "id tenant""#),
        "naming the type and both shapes: {message}",
    );
    assert!(
        message.contains("GadgetResolver"),
        "and the resolver, which is on both sides of this one: {message}",
    );
}

/// A permutation of one compound key: the same field **set**, a different string.
#[tokio::test]
async fn a_permuted_compound_key_is_the_same_claim_twice() {
    let err = TestApp::for_module::<PermutedKeyModule>()
        .await
        .err()
        .expect("one field set claimed twice is one claim too many");
    let message = err.to_string();

    assert!(
        message.contains(r#"entity "Doodad" keyed by"#)
            && message.contains("tenant")
            && message.contains("PermutedKeyResolver"),
        "naming the type, the shapes and the resolver: {message}",
    );
}

#[derive(SimpleObject)]
struct Doodad {
    id: i32,
    tenant: String,
    label: String,
}

#[resolver]
struct PermutedKeyResolver;

#[operations]
impl PermutedKeyResolver {
    #[entity]
    #[public]
    async fn find_doodad_by_id_and_tenant(&self, id: i32, tenant: String) -> Result<Doodad> {
        Ok(Doodad {
            id,
            tenant,
            label: "declared first".into(),
        })
    }

    #[entity]
    #[public]
    async fn find_doodad_by_tenant_and_id(&self, tenant: String, id: i32) -> Result<Doodad> {
        Ok(Doodad {
            id,
            tenant,
            label: "declared second".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [PermutedKeyResolver])]
struct PermutedKeyModule;

/// Apollo's key syntax nests: `"org { id }"` selects one top-level field, so
/// it shares no field with `"id"`.
#[tokio::test]
async fn a_nested_key_selection_is_one_top_level_field() {
    use nest_rs_graphql::async_graphql::InputObject;

    #[derive(InputObject)]
    struct OrgRef {
        id: i32,
    }

    #[derive(SimpleObject)]
    struct Doohickey {
        id: i32,
        label: String,
    }

    #[resolver]
    struct DooResolver;

    #[operations]
    impl DooResolver {
        #[entity]
        #[public]
        async fn find_doohickey_by_id(&self, id: i32) -> Result<Doohickey> {
            Ok(Doohickey {
                id,
                label: "by id".into(),
            })
        }

        #[entity]
        #[public]
        async fn find_doohickey_by_org(&self, org: OrgRef) -> Result<Doohickey> {
            Ok(Doohickey {
                id: org.id,
                label: "by org".into(),
            })
        }
    }

    #[module(imports = [subgraph()], providers = [DooResolver])]
    struct DooModule;

    let app = TestApp::for_module::<DooModule>()
        .await
        .expect("`id` and `org { id }` share no top-level field");

    let sdl = service_sdl(&app).await;
    assert!(
        sdl.contains(r#"@key(fields: "id")"#) && sdl.contains("org {"),
        "both keys are published, neither refused: {sdl}",
    );
}

#[derive(SimpleObject)]
struct Gadget {
    id: i32,
    tenant: String,
    label: String,
}

#[resolver]
struct GadgetResolver;

#[operations]
impl GadgetResolver {
    #[entity]
    #[public]
    async fn find_gadget_by_id(&self, id: i32) -> Result<Gadget> {
        Ok(Gadget {
            id,
            tenant: "any".into(),
            label: "by id".into(),
        })
    }

    #[entity]
    #[public]
    async fn find_gadget_by_id_and_tenant(&self, id: i32, tenant: String) -> Result<Gadget> {
        Ok(Gadget {
            id,
            tenant,
            label: "by id and tenant".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [GadgetResolver])]
struct OneResolverTwoKeysModule;

/// An `#[entity]` whose resolved type is not a GraphQL **object** keys nothing:
/// `Registry::add_keys` returns silently for a list, a scalar or a union.
#[tokio::test]
async fn an_entity_that_resolves_to_a_list_fails_the_boot() {
    let err = TestApp::for_module::<ListEntityModule>()
        .await
        .err()
        .expect("an `#[entity]` async-graphql keys nothing on is refused");
    let message = err.to_string();

    assert!(
        message.contains("ListEntityResolver::find_widgets_by_id"),
        "the boot error names the resolver and the method: {message}",
    );
    assert!(
        message.contains("[Widget!]"),
        "and the type it actually resolves to, which is the fact: {message}",
    );
}

#[resolver]
struct ListEntityResolver;

#[operations]
impl ListEntityResolver {
    #[entity]
    #[public]
    async fn find_widgets_by_id(&self, id: i32) -> Result<Vec<Widget>> {
        Ok(vec![Widget {
            id,
            label: "never keyed".into(),
        }])
    }
}

#[module(imports = [subgraph()], providers = [ListEntityResolver])]
struct ListEntityModule;

/// A scalar *is* registered, just not as an object, so the check is "did it
/// come back keyed".
#[tokio::test]
async fn an_entity_that_resolves_to_a_scalar_fails_the_boot() {
    let err = TestApp::for_module::<ScalarEntityModule>()
        .await
        .err()
        .expect("an `#[entity]` resolving to a scalar is refused");
    let message = err.to_string();

    assert!(
        message.contains("ScalarEntityResolver::find_count_by_id") && message.contains("Int"),
        "naming the method and the scalar it resolves to: {message}",
    );
}

#[resolver]
struct ScalarEntityResolver;

#[operations]
impl ScalarEntityResolver {
    #[entity]
    #[public]
    async fn find_count_by_id(&self, id: i32) -> Result<i32> {
        Ok(id)
    }
}

#[module(imports = [subgraph()], providers = [ScalarEntityResolver])]
struct ScalarEntityModule;

/// `Option<T>` reports `T`'s own type name, so it keys as the bare object does.
#[tokio::test]
async fn an_entity_returning_an_optional_object_still_keys() {
    let app = TestApp::for_module::<OptionalEntityModule>()
        .await
        .expect("`Result<Option<T>>` is the documented shape for a missing reference");

    let body = entities(&app, 5).await;
    assert_eq!(
        body["data"]["_entities"][0]["label"], "optional",
        "and it resolves by reference like any other entity: {body}",
    );
}

#[resolver]
struct OptionalEntityResolver;

#[operations]
impl OptionalEntityResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Option<Widget>> {
        Ok(Some(Widget {
            id,
            label: "optional".into(),
        }))
    }
}

#[module(imports = [subgraph()], providers = [OptionalEntityResolver])]
struct OptionalEntityModule;

// The `_entities` fan-out, and what async-graphql does with a batch it cannot
// fully resolve — **upstream** behaviour, frozen so a version bump shows it.

/// No other limit sees the length of the `representations` list.
#[tokio::test]
async fn a_reference_list_over_the_ceiling_is_refused_naming_it() {
    let app = TestApp::for_module::<CappedModule>()
        .await
        .expect("a subgraph with a pinned ceiling boots");

    let under = entities_for(&app, (0..3).map(representation).collect()).await;
    assert_eq!(
        under["data"]["_entities"].as_array().map(Vec::len),
        Some(3),
        "a call at the ceiling resolves: {under}",
    );

    let over = entities_for(&app, (0..4).map(representation).collect()).await;
    assert!(
        over["data"].is_null(),
        "one over is refused rather than truncated — a router handed fewer \
         entities than it asked for renders a page with holes: {over}",
    );
    let message = over["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains('4') && message.contains('3'),
        "the error names the count and the ceiling: {over}",
    );
    assert!(
        message.contains("MAX_REPRESENTATIONS"),
        "and the key that moves it, built rather than spelled: {over}",
    );
}

/// `0` is the unlimited sentinel pinned in code too, which never passes
/// through `ConfigService::count`.
#[tokio::test]
async fn a_ceiling_of_zero_is_unlimited() {
    let app = TestApp::for_module::<UncappedModule>()
        .await
        .expect("a subgraph with the ceiling disabled boots");

    let body = entities_for(&app, (0..50).map(representation).collect()).await;
    assert_eq!(
        body["data"]["_entities"].as_array().map(Vec::len),
        Some(50),
        "no ceiling, no refusal: {body}",
    );
}

/// The env var drives the field, checked on the value the endpoint serves.
#[test]
fn the_env_var_drives_the_ceiling() {
    use nest_rs_config::{Config, ConfigService};

    let cfg = GraphqlConfig::from_env(
        &ConfigService::with_vars("graphql", [("MAX_REPRESENTATIONS", "7")]),
        GraphqlConfig::default(),
    )
    .expect("the overlay resolves");
    assert_eq!(cfg.max_representations, Some(7));

    let unlimited = GraphqlConfig::from_env(
        &ConfigService::with_vars("graphql", [("MAX_REPRESENTATIONS", "0")]),
        GraphqlConfig::default(),
    )
    .expect("the overlay resolves");
    assert_eq!(unlimited.max_representations, None, "`0` ⇒ unlimited");
}

fn capped(max: Option<usize>) -> nest_rs_graphql::GraphqlSetup {
    GraphqlModule::for_root(GraphqlConfig {
        federation: true,
        max_representations: max,
        ..GraphqlConfig::default()
    })
}

#[resolver]
struct CappedResolver;

#[operations]
impl CappedResolver {
    #[query]
    #[public]
    async fn widget(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "queried".into(),
        })
    }

    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "capped".into(),
        })
    }
}

#[module(imports = [capped(Some(3))], providers = [CappedResolver])]
struct CappedModule;

#[resolver]
struct UncappedResolver;

#[operations]
impl UncappedResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "uncapped".into(),
        })
    }
}

#[module(imports = [capped(Some(0))], providers = [UncappedResolver])]
struct UncappedModule;

/// **Upstream behaviour, frozen.** async-graphql resolves the batch with
/// `try_join_all`, so one unmatched reference discards the whole response.
#[tokio::test]
async fn one_unresolvable_reference_discards_the_whole_batch() {
    let app = TestApp::for_module::<UncappedModule>()
        .await
        .expect("a subgraph boots");

    let body = entities_for(
        &app,
        vec![
            representation(1),
            serde_json::json!({ "__typename": "NotAType", "id": 2 }),
            representation(3),
        ],
    )
    .await;

    assert!(
        body["data"].is_null(),
        "the two resolvable references are discarded with the third: {body}",
    );
    assert_eq!(body["errors"][0]["message"], "Entity not found.");
}

/// **Upstream, frozen**: one sentence for four causes, three of them input
/// errors.
#[tokio::test]
async fn a_malformed_reference_is_reported_as_a_missing_one() {
    let app = TestApp::for_module::<UncappedModule>()
        .await
        .expect("a subgraph boots");

    for (case, representation) in [
        (
            "an unknown __typename",
            serde_json::json!({ "__typename": "NotAType", "id": 1 }),
        ),
        (
            "a missing key field",
            serde_json::json!({ "__typename": "Widget" }),
        ),
        (
            "a key of the wrong type",
            serde_json::json!({ "__typename": "Widget", "id": "abc" }),
        ),
        (
            "a representation that is not an object",
            serde_json::json!("Widget"),
        ),
    ] {
        let body = entities_for(&app, vec![representation]).await;
        assert_eq!(
            body["errors"][0]["message"], "Entity not found.",
            "{case} reads as a failed lookup: {body}",
        );
    }

    let body = entities_for(&app, vec![serde_json::json!({ "__typename": 7 })]).await;
    assert_eq!(
        body["errors"][0]["message"], "\"__typename\" must be an existing string.",
        "the one malformation that gets its own sentence: {body}",
    );
}

/// A resolver answering `Ok(None)` produces a `null` element with no error:
/// the non-oracle the posture rules ask for, not to be "improved".
#[tokio::test]
async fn a_reference_that_resolves_to_nothing_is_a_null_without_an_error() {
    let app = TestApp::for_module::<MissingEntityModule>()
        .await
        .expect("a subgraph boots");

    let body = entities_for(&app, vec![representation(1)]).await;
    assert!(
        body["data"]["_entities"][0].is_null(),
        "the element is null: {body}",
    );
    assert!(
        body["errors"].is_null(),
        "and nothing distinguishes it from a reference the caller may not read: {body}",
    );
}

#[resolver]
struct MissingEntityResolver;

#[operations]
impl MissingEntityResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, _id: i32) -> Result<Option<Widget>> {
        Ok(None)
    }
}

#[module(imports = [subgraph()], providers = [MissingEntityResolver])]
struct MissingEntityModule;

/// **The gate and the entity site's subtraction must agree**: `GuardSpecs` can
/// be seeded without `use_guards_global`, which alone seeds the `FederationGate`.
#[tokio::test]
async fn a_pool_seeded_without_the_gate_still_reaches_an_entity() {
    let app = TestApp::builder()
        .module::<PooledModule>()
        .provide(GuardSpecs(vec![guard::<Closed>()]))
        .build()
        .await
        .expect("a pool seeded directly boots");

    let ordinary = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ widget(id: 1) { id } }" }))
        .send()
        .await;
    let ordinary = ordinary
        .json()
        .await
        .value()
        .deserialize::<serde_json::Value>();

    let body = entities(&app, 7).await;
    assert!(
        body.to_string().contains("not for this router"),
        "an entity is gated by the same pool that gates an ordinary operation \
         ({ordinary}) — the site subtracts the pool only where the gate is \
         installed to have run it: {body}",
    );
    assert_eq!(
        UNGUARDED_ENTITY_RUNS.load(Ordering::SeqCst),
        0,
        "and the body never ran: {body}",
    );
}

/// **A guard declared at both scopes runs once, not once per representation**
/// — `demo`'s own shape: a global pool plus `#[use_guards]` on the resolver.
#[tokio::test]
async fn a_guard_declared_at_both_scopes_checks_an_entities_operation_once() {
    let app = TestApp::builder()
        .module::<DoublyDeclaredModule>()
        .use_guards_global([guard::<Counting>()])
        .build()
        .await
        .expect("a resolver re-declaring a pooled guard boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": REFERENCE,
            "variables": { "reps": [representation(1), representation(2), representation(3)] },
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert_eq!(
        body["data"]["_entities"].as_array().map(Vec::len),
        Some(3),
        "the three references resolve: {body}",
    );
    assert_eq!(
        COUNTED_CHECKS.load(Ordering::SeqCst),
        1,
        "broadest scope wins, and it ran at the gate: {body}",
    );
}

#[resolver]
#[use_guards(Counting)]
struct DoublyDeclaredResolver;

#[operations]
impl DoublyDeclaredResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> Result<Widget> {
        Ok(Widget {
            id,
            label: "counted".into(),
        })
    }
}

#[module(imports = [subgraph()], providers = [DoublyDeclaredResolver, Counting])]
struct DoublyDeclaredModule;

/// **The ceiling bounds the operation, not the field**: `_entities` may be
/// aliased without limit.
#[tokio::test]
async fn aliased_entity_calls_are_counted_together() {
    let app = TestApp::for_module::<CappedModule>()
        .await
        .expect("a subgraph with a pinned ceiling boots");

    let aliases = (0..3)
        .map(|index| {
            format!("a{index}: _entities(representations: $reps) {{ ... on Widget {{ label }} }}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": format!("query($reps: [_Any!]!) {{ {aliases} }}"),
            "variables": { "reps": [representation(1), representation(2)] },
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert!(
        body["data"].is_null(),
        "three aliases of two references is six, over the ceiling of three: {body}",
    );
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains('6') && message.contains('3')),
        "and the error names the total and the ceiling: {body}",
    );
}

/// The ceiling is charged to the operation the request **selected**.
#[tokio::test]
async fn an_unselected_operation_does_not_trip_the_ceiling() {
    let app = TestApp::for_module::<CappedModule>()
        .await
        .expect("a subgraph with a pinned ceiling boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query Cheap { widget(id: 1) { id } } \
                      query Fat($reps: [_Any!]!) { _entities(representations: $reps) \
                      { ... on Widget { label } } }",
            "operationName": "Cheap",
            "variables": { "reps": [representation(1), representation(2), representation(3),
                                    representation(4)] },
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert_eq!(
        body["data"]["widget"]["id"], 1,
        "the selected operation runs; the ceiling belongs to the one that does: {body}",
    );
}
