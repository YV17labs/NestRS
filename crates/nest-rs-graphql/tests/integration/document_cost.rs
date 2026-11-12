//! Covers `src/document_cost.rs`: what a document's fragments multiply and its
//! arguments copy and scan is refused before async-graphql walks it, as a document
//! past the limit is refused, on every path a document reaches the schema by.
//!
//! The walk it stands in front of is synchronous, so no test here can time out of a
//! regression: each proves its order by the answer, which a walk reaching its end
//! changes, on a document that costs nothing to walk, before any that would not end.

use async_graphql::futures_util::stream::{self, Stream};
use async_graphql::{Object, SimpleObject};
use nest_rs_config::Surface;
use nest_rs_core::{Module, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};

use crate::{fan_out, next_message};

#[derive(SimpleObject)]
struct Leaf {
    label: String,
}

/// A row of a list, each of which reads its field's argument again.
struct Row;

#[Object]
impl Row {
    async fn read(&self, x: Option<String>) -> usize {
        x.map_or(0, |x| x.len())
    }

    async fn inner(&self) -> Leaf {
        Leaf {
            label: "inner".into(),
        }
    }

    async fn more(&self) -> Vec<Row> {
        vec![Row, Row]
    }
}

/// How many rows `rows` answers.
const ROWS: usize = 3;

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
    async fn rows(&self) -> Vec<Row> {
        Vec::from_fn(ROWS, |_| Row)
    }

    #[query]
    #[public]
    async fn row(&self) -> Option<Row> {
        Some(Row)
    }

    #[mutation]
    #[public]
    async fn put(&self, x: Option<String>) -> usize {
        x.map_or(0, |x| x.len())
    }

    #[query]
    #[public]
    async fn echo(&self, values: Option<Vec<String>>) -> usize {
        values.map_or(0, |values| values.len())
    }

    #[query]
    #[public]
    async fn numbers(&self, values: Option<Vec<i32>>) -> usize {
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

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_list_argument_bytes: None,
        ..Default::default()
    }),
    ExpansionFeatureModule,
])]
struct AppRowsUnbounded;

/// A bound pinned under the default.
const PINNED_ROW_BYTES: usize = 100;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_list_argument_bytes: Some(PINNED_ROW_BYTES),
        ..Default::default()
    }),
    ExpansionFeatureModule,
])]
struct AppRowsPinned;

/// The body cap the promise of a variable's two references is read at, small enough
/// for a list of numbers filling it to cost a test nothing.
const SMALL_BODY: usize = 64 * 1024;

#[module(imports = [
    HttpModule::for_root(HttpConfig {
        max_body_bytes: SMALL_BODY,
        ..HttpConfig::default()
    }),
    GraphqlModule,
    ExpansionFeatureModule,
])]
struct AppSmallBody;

/// A socket's message cap past half the limit a POST reads by default.
const LARGE_MESSAGE: usize = 3 * 1024 * 1024;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_message_bytes: LARGE_MESSAGE,
        ..Default::default()
    }),
    ExpansionFeatureModule,
])]
struct AppLargeMessage;

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
        "every variable may be referred to twice"
    );
}

/// A request filling the body cap with a variable may refer to it twice, whatever the
/// variable holds: a list of small numbers weighs what its text does, not what its
/// values take in memory.
#[tokio::test]
async fn every_variable_may_be_referred_to_twice_whatever_it_holds() {
    let app = boot::<AppSmallBody>().await;
    let refused = answer_past_the_limit(&app).await;
    let references = |count: usize, field: &str, kind: &str| {
        let fields: String = (0..count)
            .map(|at| format!("a{at}: {field}(values: $v) "))
            .collect();
        format!("query($v: {kind}) {{ {fields}}}")
    };
    let numbers = vec![1; (SMALL_BODY - 200) / 2];
    let strings = vec!["x".repeat(SMALL_BODY - 300)];
    for (document, variable) in [
        (references(2, "numbers", "[Int]"), json!(numbers)),
        (references(2, "echo", "[String]"), json!(strings)),
    ] {
        let body = json!({ "query": document, "variables": { "v": variable } });
        assert!(body.to_string().len() <= SMALL_BODY, "within the body cap");
        let answered = post(&app, &body).await;
        assert!(
            answered.get("errors").is_none(),
            "{:.300}",
            answered.to_string()
        );
    }
    let thrice = json!({
        "query": references(3, "numbers", "[Int]"),
        "variables": { "v": numbers },
    });
    assert_eq!(post(&app, &thrice).await, refused);
}

/// A reference copies a default as it copies what the request sent.
#[tokio::test]
async fn a_default_copies_at_each_reference() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let default = "x".repeat(100_000);
    let echo = |references: usize| {
        let list = vec!["$v"; references].join(",");
        format!(r#"query($v: String = "{default}") {{ echo(values: [{list}]) }}"#)
    };
    assert_eq!(answer(&app, &echo(100)).await, refused);
    assert_eq!(answer(&app, &echo(2)).await["data"]["echo"], 2);
}

/// What a request sends `$v` as, for `document`.
fn sending(document: &str, value: &str) -> Value {
    json!({ "query": document, "variables": { "v": value } })
}

/// Past the default bound by a little, within it by a little.
const PAST_THE_BOUND: usize = 5_000;
const WITHIN_THE_BOUND: usize = 1_000;

/// async-graphql reads a field's arguments again at each row of a list above it,
/// copying the variable each time (`Context::var_value`'s `.cloned()`), so a 1 MiB
/// value over 1,000 rows peaked past a gigabyte with no credential. What the fields
/// below a list read is held to a bound before anything runs, however the document
/// reaches them.
#[tokio::test]
async fn what_the_rows_of_a_list_copy_is_held_to_a_bound() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let past = "x".repeat(PAST_THE_BOUND);
    for document in [
        "query($v: String) { rows { read(x: $v) } }",
        "query($v: String) { rows { inner { label } more { read(x: $v) } } }",
        "query($v: String) { row { more { read(x: $v) } } }",
        "query($v: String) { rows { ... on Row { read(x: $v) } } }",
        "query($v: String) { rows { ...R } } fragment R on Row { read(x: $v) }",
        "query($v: String) { rows { ...M } } fragment M on Row { more { read(x: $v) } }",
        "query($v: String) { row { ...L } } fragment L on Row { more { read(x: $v) } }",
    ] {
        assert_eq!(
            post(&app, &sending(document, &past)).await,
            refused,
            "{document}"
        );
    }
    let literal = format!(r#"{{ rows {{ read(x: "{past}") }} }}"#);
    assert_eq!(
        answer(&app, &literal).await,
        refused,
        "a literal is copied too"
    );

    let within = "x".repeat(WITHIN_THE_BOUND);
    let answered = post(
        &app,
        &sending("query($v: String) { rows { read(x: $v) } }", &within),
    )
    .await;
    assert_eq!(
        answered["data"]["rows"],
        json!(vec![json!({ "read": WITHIN_THE_BOUND }); ROWS]),
        "{answered:.300}"
    );
}

/// The bound is `max_list_argument_bytes`: pinned, it is read at its byte, and off,
/// a list's rows read what they are sent.
#[tokio::test]
async fn the_bound_is_what_max_list_argument_bytes_says() {
    let document = "query($v: String) { rows { read(x: $v) } }";
    let pinned = boot::<AppRowsPinned>().await;
    let refused = answer_past_the_limit(&pinned).await;
    // The reference stands in for a `null` and scans one definition.
    let within = "x".repeat(PINNED_ROW_BYTES - 2);
    let answered = post(&pinned, &sending(document, &within)).await;
    assert_eq!(
        answered["data"]["rows"][0]["read"],
        within.len(),
        "{answered}"
    );
    let past = "x".repeat(PINNED_ROW_BYTES - 1);
    assert_eq!(post(&pinned, &sending(document, &past)).await, refused);

    let unbounded = boot::<AppRowsUnbounded>().await;
    let large = "x".repeat(PAST_THE_BOUND);
    let answered = post(&unbounded, &sending(document, &large)).await;
    assert_eq!(
        answered["data"]["rows"][0]["read"], PAST_THE_BOUND,
        "{answered:.300}"
    );
}

/// The socket's operations are read by the same extension.
#[tokio::test]
async fn over_graphql_ws_what_the_rows_of_a_list_copy_is_held_to_the_bound() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    let past = "x".repeat(PAST_THE_BOUND);
    socket.subscribe("rows", &format!(r#"{{ rows {{ read(x: "{past}") }} }}"#));
    let item = socket
        .next_item("rows")
        .await
        .expect("the operation answers");
    assert_eq!(item, refused);
}

/// A field no list stands above reads its arguments once, so what it reads is the
/// request's own size, which its body cap and the copies already hold.
#[tokio::test]
async fn an_argument_no_list_stands_above_is_read_once_whatever_it_weighs() {
    let app = boot::<AppDefaults>().await;
    let large = "x".repeat(500_000);
    for (document, read) in [
        (
            "mutation($v: String) { put(x: $v) }",
            json!({ "put": 500_000 }),
        ),
        (
            "query($v: String) { row { read(x: $v) } }",
            json!({ "row": { "read": 500_000 } }),
        ),
        (
            "query($v: String) { row { ...R } } fragment R on Row { read(x: $v) }",
            json!({ "row": { "read": 500_000 } }),
        ),
    ] {
        let answered = post(&app, &sending(document, &large)).await;
        assert_eq!(answered["data"], read, "{document}: {answered:.300}");
    }
}

/// A reference scans the definitions down to its own, at execution, past every limit
/// and inside one poll of the resolver.
#[tokio::test]
async fn the_definitions_a_documents_references_scan_are_held_to_what_a_request_may_carry() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let scanning = |definitions: usize, again: usize| {
        let defined: String = (0..definitions)
            .map(|at| format!(r#"$a{at}: String = "", "#))
            .collect();
        let each: String = (0..definitions).map(|at| format!("$a{at}, ")).collect();
        let last = "$z, ".repeat(again);
        format!(r#"query({defined}$z: String = "") {{ echo(values: [{each}{last}$z]) }}"#)
    };
    assert_eq!(answer(&app, &scanning(3_000, 3_000)).await, refused);
    assert_eq!(answer(&app, &scanning(100, 100)).await["data"]["echo"], 201);
}

/// A repeated key's arguments look each other up, ahead of every limit. A document past
/// that hears the limit, where validation would have paid the square to list what it
/// holds; the arguments a schema declares never come near it.
#[tokio::test]
async fn a_key_repeated_with_thousands_of_arguments_is_refused_unread() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let arguments: String = (0..3_000).map(|at| format!("a{at}: 1, ")).collect();
    let document = format!("{{ echo({arguments}) echo({arguments}) }}");
    assert_eq!(answer(&app, &document).await, refused);

    let repeated = r#"{ echo(values: ["a"]) echo(values: ["a"]) }"#;
    assert_eq!(answer(&app, repeated).await["data"]["echo"], 1);
}

/// An operation the request's name leaves out copies nothing of its own: each of these
/// refers once to a value only a document of one operation could copy three times.
#[tokio::test]
async fn an_operation_a_name_leaves_out_is_not_counted() {
    let app = boot::<AppDefaults>().await;
    let value = "x".repeat(1_500_000);
    let operations: String = ["A", "B", "C"]
        .map(|name| format!("query {name}($v: String) {{ echo(values: [$v]) }} "))
        .concat();
    let body = json!({ "query": operations, "operationName": "A", "variables": { "v": value } });
    assert_eq!(post(&app, &body).await["data"]["echo"], 1);
}

/// What async-graphql copies whichever operation runs: a fragment's arguments, by what
/// validates them, and with no name to leave one out, every operation's.
#[tokio::test]
async fn a_fragment_and_a_document_with_no_name_are_counted_whichever_runs() {
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;
    let value = "x".repeat(100_000);
    let list = vec!["$v"; 100].join(",");
    let direct =
        format!("query A {{ __typename }} query B($v: String) {{ echo(values: [{list}]) }}");
    let spread = format!(
        "query A {{ __typename }} query B($v: String) {{ ...F }} \
         fragment F on Query {{ echo(values: [{list}]) }}"
    );
    let selecting = |document: &str, name: Option<&str>| {
        json!({
            "query": document,
            "operationName": name,
            "variables": { "v": value },
        })
    };

    let answered = post(&app, &selecting(&direct, Some("A"))).await;
    assert_eq!(answered["data"]["__typename"], "Query", "{answered}");
    assert_eq!(post(&app, &selecting(&spread, Some("A"))).await, refused);
    assert_eq!(post(&app, &selecting(&direct, None)).await, refused);
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

/// `document`'s first answer over a socket of `M`, by a real upgrade, the one path that
/// holds an operation to its socket's message cap.
async fn over_a_real_socket<M: Module + 'static>(document: &str) -> Value {
    let app = TestApp::builder()
        .module::<M>()
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut socket = app
        .socket("/graphql")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .connect()
        .await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    let subscribe = json!({ "id": "1", "type": "subscribe", "payload": { "query": document } });
    socket.send_text(subscribe.to_string()).await;
    let answered = next_message(&mut socket).await;
    app.shutdown().await.expect("the transport stops cleanly");
    answered
}

/// What a socket's operation copies is held to twice its own message cap, as a POST's
/// is to twice its body cap: what a POST may copy here, a socket of the default cap
/// may not, and one raised past that POST's limit may.
#[tokio::test]
async fn over_graphql_ws_copies_are_held_to_twice_the_message_cap() {
    let copying = |spreads: usize, literal: usize| {
        format!(
            r#"{{ {} }} fragment F on Query {{ echo(values: ["{}"]) }}"#,
            "...F ".repeat(spreads),
            "x".repeat(literal),
        )
    };
    let app = boot::<AppDefaults>().await;
    let refused = answer_past_the_limit(&app).await;

    let past_the_default_cap = copying(100, 2_000);
    assert_eq!(answer(&app, &past_the_default_cap).await["data"]["echo"], 1);
    let answered = over_a_real_socket::<AppDefaults>(&past_the_default_cap).await;
    assert_eq!(answered["payload"], refused, "{answered}");

    let past_a_posts_limit = copying(50, 100_000);
    assert_eq!(answer(&app, &past_a_posts_limit).await, refused);
    let answered = over_a_real_socket::<AppLargeMessage>(&past_a_posts_limit).await;
    assert_eq!(answered["payload"]["data"]["echo"], 1, "{answered}");
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
