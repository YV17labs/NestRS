//! Covers `src/document_cost.rs`: what a document's fragments multiply and its
//! arguments copy is refused before async-graphql walks it, as a document past the
//! limit is refused, on every path a document reaches the schema by.
//!
//! The walk it stands in front of is synchronous, so no test here can time out of a
//! regression: each proves its order by the answer, which a walk reaching its end
//! changes, on a document that costs nothing to walk, before any that would not end.

use async_graphql::SimpleObject;
use async_graphql::futures_util::stream::{self, Stream};
use nest_rs_config::Surface;
use nest_rs_core::{Module, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};

#[derive(SimpleObject)]
struct Leaf {
    label: String,
}

#[resolver]
struct ExpansionResolver;

#[operations]
impl ExpansionResolver {
    #[query]
    #[public]
    async fn root(&self) -> Leaf {
        Leaf {
            label: "root".into(),
        }
    }

    #[query]
    #[public]
    async fn echo(&self, values: Option<Vec<String>>) -> usize {
        values.map_or(0, |values| values.len())
    }

    #[subscription]
    #[public]
    async fn ticks(&self) -> impl Stream<Item = i32> {
        stream::iter([1])
    }
}

#[module(providers = [ExpansionResolver])]
struct ExpansionFeatureModule;

#[module(imports = [GraphqlModule, ExpansionFeatureModule])]
struct AppDefaults;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_complexity: Some(10),
        ..Default::default()
    }),
    ExpansionFeatureModule,
])]
struct AppLowLimit;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_depth: None,
        max_complexity: None,
        ..Default::default()
    }),
    ExpansionFeatureModule,
])]
struct AppLimitsOff;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        introspection: Surface::Open,
        ..Default::default()
    }),
    ExpansionFeatureModule,
])]
struct AppIntrospected;

async fn boot<M: Module + 'static>() -> TestApp {
    TestApp::builder()
        .module::<M>()
        .build()
        .await
        .expect("the schema boots")
}

async fn post(app: &TestApp, body: &Value) -> Value {
    let resp = app.http().post("/graphql").body_json(body).send().await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

async fn answer(app: &TestApp, query: &str) -> Value {
    post(app, &json!({ "query": query })).await
}

/// A document holding no fragment, past the default limit by itself: what
/// async-graphql's own visitor answers.
async fn answer_past_the_limit(app: &TestApp) -> Value {
    let default = GraphqlConfig::default()
        .max_complexity
        .expect("on by default");
    let wide: String = (0..=default)
        .map(|at| format!("a{at}: root {{ __typename label }} "))
        .collect();
    let answered = answer(app, &format!("{{ {wide}}}")).await;
    assert_eq!(answered["data"], Value::Null, "{answered}");
    answered
}

/// `root { ...F0 }` under `operation`, each fragment on `on` spreading the next
/// twice, the last holding `leaf`.
fn fan_out(operation: &str, on: &str, depth: usize, leaf: &str) -> String {
    let mut document = format!("{operation} {{ ...F0 }}");
    for at in 0..depth {
        document.push_str(&format!(
            " fragment F{at} on {on} {{ ...F{next} ...F{next} }}",
            next = at + 1
        ));
    }
    document.push_str(&format!(" fragment F{depth} on {on} {{ {leaf} }}"));
    document
}

fn query_fan_out(depth: usize, leaf: &str) -> String {
    fan_out("query", "Query", depth, leaf)
}

/// The deepest fan-out every leaf below keeps within the recursion bound.
const DEEPEST: usize = 30;

/// Every leaf a fan-out can end in, among them what async-graphql's score counts
/// nothing for: a `__typename`, which it answers and resolves `2^depth` times, and
/// an unknown fragment, which it refuses only after the walks.
const LEAVES: [&str; 4] = [
    "root { label }",
    "__typename",
    "...Missing",
    "... { __typename }",
];

#[tokio::test]
async fn a_fan_out_is_refused_as_a_document_past_the_limit_is() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    for depth in [14, DEEPEST] {
        for leaf in LEAVES {
            assert_eq!(
                answer(&app, &query_fan_out(depth, leaf)).await,
                refused,
                "{depth}: {leaf}",
            );
        }
    }
}

/// `check_recursive_depth` reads the fan-out on its way to what follows it, past its
/// bound, and says what it found there: answering first means it never started.
#[tokio::test]
async fn a_fan_out_is_refused_before_the_recursion_bound_reads_it() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let deep = (0..40).fold(String::from("__typename"), |inner, _| {
        format!("... {{ {inner} }}")
    });
    let document =
        query_fan_out(14, "__typename").replacen("{ ...F0 }", &format!("{{ ...F0 {deep} }}"), 1);
    assert_eq!(answer(&app, &document).await, refused);
}

#[tokio::test]
async fn the_limit_is_what_max_complexity_says() {
    let app = boot::<AppLowLimit>().await;
    let spreads = |count: usize| {
        format!(
            "{{ {} }} fragment A on Query {{ __typename }}",
            "...A ".repeat(count),
        )
    };
    let admitted = answer(&app, &spreads(11)).await;
    assert_eq!(admitted["data"]["__typename"], "Query", "{admitted}");
    let refused = answer(&app, &spreads(12)).await;
    assert_eq!(
        refused["errors"][0]["message"], "Query is too complex.",
        "{refused}"
    );
    assert_eq!(refused["data"], Value::Null, "{refused}");
}

/// A document spreading each fragment once multiplies nothing: its score stays the
/// visitor's, whatever a count of its fields says.
#[tokio::test]
async fn a_document_spreading_each_fragment_once_is_left_to_the_score() {
    let app = boot::<AppLowLimit>().await;
    let typenames: String = (0..15).map(|at| format!("a{at}: __typename ")).collect();
    let document = format!("{{ ...A }} fragment A on Query {{ {typenames}}}");
    let answered = answer(&app, &document).await;
    assert_eq!(answered["data"]["a14"], "Query", "{answered}");
}

/// Off, `max_complexity` scores nothing, and a fragment's repeats stop at the most
/// it may be set to: the walk of a kilobyte stays bounded.
#[tokio::test]
async fn with_the_limits_off_a_fan_out_stops_at_the_most_a_limit_may_say() {
    let app = boot::<AppLimitsOff>().await;
    let small = answer(&app, &query_fan_out(6, "__typename")).await;
    assert_eq!(small["data"]["__typename"], "Query", "{small}");
    for depth in [24, DEEPEST] {
        let refused = answer(&app, &query_fan_out(depth, "__typename")).await;
        assert_eq!(
            refused["errors"][0]["message"], "Query is too complex.",
            "{depth}: {refused}",
        );
    }
}

/// async-graphql's rules read a fragment no operation reaches, whose walk its
/// recursion bound never stands in front of: a long chain of them overflows the
/// stack, which aborts the process, and one spread under thousands of others costs
/// their square.
#[tokio::test]
async fn a_fragment_no_operation_reaches_is_refused_unwalked() {
    let app = boot::<AppDefaults>().await;
    let shared: String = (0..2000).map(|at| format!(" ...B{at}")).collect();
    let bodies: String = (0..2000)
        .map(|at| format!(" fragment B{at} on Query {{ __typename }}"))
        .collect();
    let repeats: String = (0..2000)
        .map(|at| format!(" fragment U{at} on Query {{ ...H }}"))
        .collect();
    let chain: String = (0..20_000)
        .map(|at| format!(" fragment C{at} on Query {{ ...C{} }}", at + 1))
        .chain(std::iter::once(
            " fragment C20000 on Query { __typename }".to_owned(),
        ))
        .collect();
    for (document, first) in [
        (
            "{ __typename } fragment F on Query { __typename }".to_owned(),
            "F",
        ),
        (
            query_fan_out(14, "__typename").replacen("{ ...F0 }", "{ __typename }", 1),
            "F0",
        ),
        (
            format!("{{ __typename }} fragment H on Query {{{shared} }}{bodies}{repeats}"),
            "H",
        ),
        (format!("{{ __typename }}{chain}"), "C0"),
    ] {
        let refused = answer(&app, &document).await;
        assert_eq!(refused["data"], Value::Null, "{first}");
        assert_eq!(
            refused["errors"],
            json!([{
                "message": format!(r#"Fragment "{first}" is never used"#),
                "locations": refused["errors"][0]["locations"],
            }]),
            "{first}: {:.300}",
            refused.to_string(),
        );
    }
}

/// `[$v, $v, …]` is built by copying the value at each reference, and a list scores
/// nothing, so a few kilobytes of references to a large value cost gigabytes.
#[tokio::test]
async fn the_copies_a_documents_references_make_are_held_to_what_a_request_may_carry() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let value = "x".repeat(100_000);
    let echo = |references: usize, value: &str| {
        let list = vec!["$v"; references].join(",");
        json!({
            "query": format!("query($v: String) {{ echo(values: [{list}]) }}"),
            "variables": { "v": value },
        })
    };

    assert_eq!(post(&app, &echo(100, &value)).await, refused);

    let answered = post(&app, &echo(40, &value)).await;
    assert_eq!(answered["data"]["echo"], 40, "{answered}");
    let answered = post(&app, &echo(2_000, "short")).await;
    assert_eq!(answered["data"]["echo"], 2_000, "{answered}");
    let largest = "x".repeat(900_000);
    let answered = post(&app, &echo(2, &largest)).await;
    assert_eq!(
        answered["data"]["echo"], 2,
        "any variable may be referred to twice"
    );
}

/// A fragment's arguments are copied again at each of its spreads, its text's and its
/// references alike.
#[tokio::test]
async fn a_fragment_spread_again_copies_its_arguments_again() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let text = "x".repeat(100_000);
    let spreads = "...F ".repeat(100);
    let document = format!(r#"{{ {spreads}}} fragment F on Query {{ echo(values: ["{text}"]) }}"#);
    assert_eq!(answer(&app, &document).await, refused);
    let document =
        format!(r#"query($v: String) {{ {spreads}}} fragment F on Query {{ echo(values: [$v]) }}"#);
    let body = json!({ "query": document, "variables": { "v": text } });
    assert_eq!(post(&app, &body).await, refused);
}

/// A cycle a document reaches is the recursion bound's to answer, in one descent.
#[tokio::test]
async fn a_cycle_is_answered_by_the_recursion_bound() {
    let app = boot::<AppDefaults>().await;
    for document in [
        "{ ...F } fragment F on Query { ...F ...F }",
        "{ ...F } fragment F on Query { ...G ...G } fragment G on Query { ...F ...F }",
    ] {
        let refused = answer(&app, document).await;
        assert_eq!(
            refused["errors"][0]["message"],
            "The recursion depth of the query cannot be greater than `32`",
            "{document}: {refused}",
        );
    }
}

/// What a client writes: one fragment under several parents, and the document every
/// introspecting tool sends, which repeats a type reference a dozen times.
#[tokio::test]
async fn a_document_using_its_fragments_as_clients_do_is_answered() {
    let app = boot::<AppIntrospected>().await;
    let reused = answer(
        &app,
        "{ a: root { ...L } b: root { ...L } c: root { ...L __typename } } \
         fragment L on Leaf { label ...T } fragment T on Leaf { __typename }",
    )
    .await;
    assert!(reused.get("errors").is_none(), "{reused}");
    assert_eq!(reused["data"]["c"]["label"], "root", "{reused}");

    let introspected = answer(&app, INTROSPECTION).await;
    assert!(introspected.get("errors").is_none(), "{introspected}");
    assert_eq!(
        introspected["data"]["__schema"]["queryType"]["name"], "Query",
        "{introspected}"
    );
}

#[tokio::test]
async fn a_batch_refuses_only_the_operation_fanning_out() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let answered = post(
        &app,
        &json!([
            { "query": query_fan_out(14, "__typename") },
            { "query": "{ root { label } }" },
        ]),
    )
    .await;
    assert_eq!(answered[0], refused, "{answered}");
    assert_eq!(answered[1]["data"]["root"]["label"], "root", "{answered}");
}

/// A socket's operation is refused as a POST's, and the socket serves the next.
#[tokio::test]
async fn over_graphql_ws_a_fan_out_is_refused_and_the_socket_stays() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    for (id, document) in [
        ("query", query_fan_out(14, "__typename")),
        (
            "subscription",
            fan_out("subscription", "Subscription", 14, "ticks"),
        ),
        ("deep", query_fan_out(DEEPEST, "__typename")),
    ] {
        socket.subscribe(id, &document);
        let item = socket.next_item(id).await.expect("the operation answers");
        assert_eq!(item, refused, "{id}");
    }
    socket.subscribe("next", "{ root { label } }");
    let item = socket.next_item("next").await.expect("the socket stays");
    assert_eq!(item["data"]["root"]["label"], "root", "{item}");
}

/// Apollo's and graphql-js's introspection document, as tools send it.
const INTROSPECTION: &str = r"
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
    directives {
      name
      description
      locations
      args { ...InputValue }
    }
  }
}
fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}
fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
}
fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType {
                kind
                name
              }
            }
          }
        }
      }
    }
  }
}
";
