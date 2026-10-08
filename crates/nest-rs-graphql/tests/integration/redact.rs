//! Every error a GraphQL response carries says a decode failure without its
//! value — on the POST path and over the socket alike.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_testing::TestApp;

const SECRET: &str = "sk_live_51HsecretTOKEN";

/// What the `own_*` resolvers decode: a number, sent a secret.
const SECRET_BODY: &str = r#""sk_live_51HsecretTOKEN""#;

#[derive(nest_rs_core::serde::Deserialize, nest_rs_core::serde::Serialize)]
#[serde(crate = "nest_rs_core::serde")]
enum Kind {
    Visa,
}

#[derive(nest_rs_core::serde::Deserialize, nest_rs_core::serde::Serialize)]
#[serde(crate = "nest_rs_core::serde")]
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
    async fn own_decode(&self) -> async_graphql::Result<u64> {
        Ok(serde_json::from_str::<u64>(SECRET_BODY)?)
    }

    #[query]
    #[public]
    async fn own_anyhow(&self) -> async_graphql::Result<u64> {
        let decoded: nest_rs_core::anyhow::Result<u64> =
            serde_json::from_str::<u64>(SECRET_BODY).map_err(Into::into);
        Ok(decoded?)
    }
}

#[module(providers = [ChargeResolver])]
struct ChargeFeature;

#[module(imports = [GraphqlModule::for_root(None), ChargeFeature])]
struct ChargeApp;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ChargeApp>()
        .http(HttpTransport::new())
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
