//! Every error a GraphQL response carries says a decode failure without its
//! value — on the POST path and over the socket alike.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;

const SECRET: &str = "sk_live_51HsecretTOKEN";

/// What the `own_*` resolvers decode: a number, sent a secret.
const SECRET_BODY: &str = r#""sk_live_51HsecretTOKEN""#;

#[derive(
    nest_rs_core::__private::serde::Deserialize, nest_rs_core::__private::serde::Serialize,
)]
#[serde(crate = "nest_rs_core::__private::serde")]
enum Kind {
    Visa,
}

#[derive(
    nest_rs_core::__private::serde::Deserialize, nest_rs_core::__private::serde::Serialize,
)]
#[serde(crate = "nest_rs_core::__private::serde")]
struct Card {
    kind: Kind,
}

#[resolver]
struct ChargeResolver;

#[operations]
impl ChargeResolver {
    #[query]
    #[public]
    async fn charge(&self, amount: i32) -> i32 {
        amount
    }

    #[query]
    #[public]
    async fn charge_card(&self, card: async_graphql::Json<Card>) -> String {
        match card.0.kind {
            Kind::Visa => "visa".to_owned(),
        }
    }

    #[query]
    #[public]
    async fn own_decode(&self) -> Result<u64, serde_json::Error> {
        serde_json::from_str::<u64>(SECRET_BODY)
    }

    #[query]
    #[public]
    async fn own_anyhow(&self) -> nest_rs_core::anyhow::Result<u64> {
        Ok(serde_json::from_str::<u64>(SECRET_BODY)?)
    }

    #[query]
    #[public]
    async fn leaky(&self) -> nest_rs_core::anyhow::Result<u64> {
        Err(nest_rs_core::anyhow::anyhow!("secret 42"))
    }

    #[query]
    #[public]
    async fn deliberate(&self) -> async_graphql::Result<u64> {
        Err(async_graphql::Error::new("bad input"))
    }

    #[query]
    #[public]
    async fn conflicted(&self) -> Result<u64, Taken> {
        Err(Taken)
    }

    #[query]
    #[public]
    async fn by_role(&self, role: ProbeRole) -> bool {
        matches!(role, ProbeRole::Admin)
    }

    #[query]
    #[public]
    async fn filtered(&self, filter: ProbeFilter) -> bool {
        filter.name.is_some()
    }

    #[query]
    #[public]
    async fn coded(&self, code: ProbeCode) -> String {
        code.0
    }
}

#[derive(async_graphql::Enum, Clone, Copy, PartialEq, Eq)]
enum ProbeRole {
    Admin,
}

#[derive(async_graphql::InputObject)]
struct ProbeFilter {
    name: Option<String>,
}

/// A scalar written as async-graphql's own `UUID` is: a string it parses, its
/// refusal quoting what it read, and any other value refused as the wrong type.
struct ProbeCode(String);

#[async_graphql::Scalar]
impl async_graphql::ScalarType for ProbeCode {
    fn parse(value: async_graphql::Value) -> async_graphql::InputValueResult<Self> {
        match value {
            async_graphql::Value::String(code) if code.starts_with("code-") => Ok(Self(code)),
            async_graphql::Value::String(code) => Err(async_graphql::InputValueError::custom(
                format!("not a code: {code}"),
            )),
            other => Err(async_graphql::InputValueError::expected_type(other)),
        }
    }

    fn to_value(&self) -> async_graphql::Value {
        async_graphql::Value::String(self.0.clone())
    }
}

/// A domain error that says what its client may read.
#[derive(Debug)]
struct Taken;

impl std::fmt::Display for Taken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the handle ada@example.com is taken")
    }
}

impl std::error::Error for Taken {}

impl nest_rs_core::ToProblem for Taken {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        Some(
            nest_rs_core::Problem::new(409, nest_rs_core::problem::code::CONFLICT)
                .with_detail("the handle is taken"),
        )
    }
}

#[module(providers = [ChargeResolver])]
struct ChargeFeature;

#[module(imports = [GraphqlModule::for_root(None), ChargeFeature])]
struct ChargeApp;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ChargeApp>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

/// Each request, and the message its one error must carry.
fn cases() -> Vec<(serde_json::Value, &'static str)> {
    let report = "invalid type: a string, expected u64 at line 1 column 24";
    vec![
        (serde_json::json!({ "query": "{ ownDecode }" }), report),
        (serde_json::json!({ "query": "{ ownAnyhow }" }), report),
        (
            serde_json::json!({
                "query": "query($card: JSON!) { chargeCard(card: $card) }",
                "variables": { "card": { "kind": SECRET } },
            }),
            "Failed to parse \"JSON\": unknown variant, expected `Visa`",
        ),
        (
            serde_json::json!({
                "query": "query($amount: Int!) { charge(amount: $amount) }",
                "variables": { "amount": SECRET },
            }),
            "Invalid value for argument \"amount\", expected type \"Int\"",
        ),
    ]
}

#[tokio::test]
async fn an_error_a_response_carries_says_a_decode_failure_without_its_value() {
    let app = boot().await;
    for (request, said) in cases() {
        let resp = app.http().post("/graphql").body_json(&request).send().await;
        resp.assert_status_is_ok();
        let body: serde_json::Value = resp.json().await.value().deserialize();
        assert_eq!(body["errors"][0]["message"], said, "{request}");
        assert!(!body.to_string().contains(SECRET), "{body}");
    }
}

/// What every case below carries, and no reply may hold any part of.
const MARKER: &str = "sk_live_marker";

/// Each request carrying a marker in its `variables`, and the message its one
/// error must carry: a value, or a key, said by kind.
fn variable_cases() -> Vec<(serde_json::Value, &'static str)> {
    let enumerated = "Invalid value for argument \"role\", enumeration type \"ProbeRole\" \
                      does not contain the value sent";
    let unknown_key = "Invalid value for argument \"filter\", an unknown field of type \
                       \"ProbeFilter\"";
    vec![
        (
            serde_json::json!({
                "query": "query($role: ProbeRole!) { byRole(role: $role) }",
                "variables": { "role": "sk_live_marker_enum" },
            }),
            enumerated,
        ),
        (
            serde_json::json!({
                "query": "query($filter: ProbeFilter!) { filtered(filter: $filter) }",
                "variables": { "filter": { "sk_live_marker_key": 1 } },
            }),
            unknown_key,
        ),
        (
            serde_json::json!({
                "query": "query($filter: ProbeFilter!) { filtered(filter: $filter) }",
                "variables": { "filter": { "a\" of type \"sk_live_marker": 1 } },
            }),
            unknown_key,
        ),
        (
            serde_json::json!({
                "query": "query($code: ProbeCode!) { coded(code: $code) }",
                "variables": { "code": { "card": "sk_live_marker_scalar" } },
            }),
            "Expected input type \"ProbeCode\", found an object.",
        ),
        (
            serde_json::json!({
                "query": "query($code: ProbeCode!) { coded(code: $code) }",
                "variables": { "code": { "x\". (occurred while parsing \"sk_live_marker\")": 1 } },
            }),
            "Expected input type \"ProbeCode\", found an object.",
        ),
        (
            serde_json::json!({
                "query": "query($code: ProbeCode!) { coded(code: $code) }",
                "variables": { "code": "sk_live_marker_text" },
            }),
            "Failed to parse \"ProbeCode\": a value its type does not accept",
        ),
    ]
}

/// async-graphql's validation and input coercion quote the value a variable
/// held — an enum's, a key of an input object's, a scalar's — and a scalar's
/// own refusal may quote it too: each is said by kind.
#[tokio::test]
async fn a_variable_s_value_is_refused_without_quoting_it() {
    let app = boot().await;
    for (request, said) in variable_cases() {
        let resp = app.http().post("/graphql").body_json(&request).send().await;
        resp.assert_status_is_ok();
        let body: serde_json::Value = resp.json().await.value().deserialize();
        assert_eq!(body["errors"][0]["message"], said, "{request}");
        assert!(!body.to_string().contains(MARKER), "{body}");
    }
}

/// The socket executes through `execute_stream`; the values sit in the
/// document there, which the same sentences quote.
#[tokio::test]
async fn a_value_an_operation_sent_over_the_socket_is_refused_without_quoting_it() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    for (id, query, said) in [
        (
            "enum",
            "{ byRole(role: sk_live_marker_enum) }",
            "Invalid value for argument \"role\", enumeration type \"ProbeRole\" does not \
             contain the value sent",
        ),
        (
            "key",
            "{ filtered(filter: { sk_live_marker_key: 1 }) }",
            "Invalid value for argument \"filter\", an unknown field of type \"ProbeFilter\"",
        ),
        (
            "scalar",
            "{ coded(code: { card: \"sk_live_marker_scalar\" }) }",
            "Expected input type \"ProbeCode\", found an object.",
        ),
        (
            "text",
            "{ coded(code: \"sk_live_marker_text\") }",
            "Failed to parse \"ProbeCode\": a value its type does not accept",
        ),
    ] {
        socket.subscribe(id, query);
        let item = socket.next_item(id).await.expect("the operation answers");
        let errors = item
            .get("errors")
            .or_else(|| item.get("payload"))
            .unwrap_or_else(|| panic!("an error: {item}"));
        assert_eq!(errors[0]["message"], said, "{item}");
        assert!(!item.to_string().contains(MARKER), "{item}");
    }
}

/// An `operationName` is the request's own text, not a name its document
/// declares: one the document holds no operation of is refused by kind.
#[tokio::test]
async fn an_unknown_operation_name_is_refused_without_quoting_it() {
    let app = boot().await;
    let request = serde_json::json!({
        "query": "query A { charge(amount: 1) } query B { charge(amount: 2) }",
        "operationName": "marked free text",
    });
    let resp = app.http().post("/graphql").body_json(&request).send().await;
    resp.assert_status_is_ok();
    let body: serde_json::Value = resp.json().await.value().deserialize();
    assert_eq!(body["errors"].as_array().map(Vec::len), Some(1), "{body}");
    assert!(!body.to_string().contains("marked"), "{body}");
}

/// The socket executes through `execute_stream`, not `execute`.
#[tokio::test]
async fn an_error_an_operation_answers_over_the_socket_says_it_without_the_value() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    for (id, query) in [("decode", "{ ownDecode }"), ("anyhow", "{ ownAnyhow }")] {
        socket.subscribe(id, query);
        let item = socket.next_item(id).await.expect("the operation answers");
        assert_eq!(
            item["errors"][0]["message"],
            "invalid type: a string, expected u64 at line 1 column 24",
            "{item}",
        );
        assert!(!item.to_string().contains(SECRET), "{item}");
    }
}

async fn answer(app: &TestApp, query: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

/// An error nobody meant for the client: the constant, the `INTERNAL` code,
/// and the whole chain on the operator's line, once.
#[tokio::test]
async fn an_error_no_one_meant_for_the_client_answers_opaquely_and_files_its_chain() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;
    let body = answer(&app, "{ leaky }").await;
    assert_eq!(
        body["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "INTERNAL",
        "{body}"
    );
    assert!(!body.to_string().contains("secret"), "{body}");

    let failed = logs.expect_one(nest_rs_graphql::TARGET, "graphql operation failed");
    assert_eq!(failed.level, "error");
    assert!(
        failed
            .field("error")
            .is_some_and(|e| e.contains("secret 42")),
        "{failed:#?}",
    );
}

/// The edge's own error is the author's deliberate answer, sent as built.
#[tokio::test]
async fn a_deliberate_graphql_error_answers_as_built() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;
    let body = answer(&app, "{ deliberate }").await;
    assert_eq!(body["errors"][0]["message"], "bad input", "{body}");
    assert!(
        logs.find(nest_rs_graphql::TARGET, "graphql operation failed")
            .is_empty(),
        "an answered failure is never filed at `error`",
    );
}

/// A `ToProblem` error answers its problem: its detail and its code, never its
/// `Display`.
#[tokio::test]
async fn a_to_problem_error_answers_its_code_and_detail() {
    let app = boot().await;
    let body = answer(&app, "{ conflicted }").await;
    assert_eq!(
        body["errors"][0]["message"], "the handle is taken",
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "CONFLICT",
        "{body}"
    );
    assert!(!body.to_string().contains("ada@example.com"), "{body}");
}
