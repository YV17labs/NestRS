//! A decorated root field and subscription of an `#[operations]` block, and its
//! query, mutation, field resolver, entity and subscription attempted again.

use std::sync::Arc;

use async_graphql::futures_util::StreamExt as _;
use async_graphql::futures_util::stream::{self, BoxStream};
use nest_rs_core::module;
use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once};

#[resolver]
struct DecoratedResolver {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[operations]
impl DecoratedResolver {
    #[query]
    #[public]
    #[resilient(upstream)]
    async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }

    /// An instrument on an operation is the method's, as on a route: its
    /// `err(Debug)` records the method's own error, in either order.
    #[query]
    #[public]
    #[tracing::instrument(skip_all, err(Debug))]
    #[resilient(upstream)]
    async fn traced_above(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }

    #[query]
    #[public]
    #[resilient(upstream)]
    #[tracing::instrument(skip_all, err(Debug))]
    async fn traced_below(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }

    #[subscription]
    #[public]
    #[resilient(upstream)]
    async fn movements(&self) -> Result<BoxStream<'static, i32>, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(stream::iter([1]).boxed())
    }
}

#[module(
    imports = [GraphqlModule, ResilienceModule::for_root(opening_on_the_first_failure())],
    providers = [DecoratedResolver, Attempts],
)]
struct DecoratedGraphqlModule;

#[tokio::test(start_paused = true)]
async fn a_field_answers_its_refusal_unavailable_with_the_breakers_wait() {
    let app = TestApp::for_module::<DecoratedGraphqlModule>()
        .await
        .expect("a decorated resolver boots");
    let query = || async {
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ decorated }" }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize::<serde_json::Value>()
    };
    let failed = query().await;
    assert_eq!(
        failed["errors"][0]["extensions"]["code"], "INTERNAL",
        "{failed}"
    );

    let refused = query().await;
    let extensions = &refused["errors"][0]["extensions"];
    assert_eq!(extensions["code"], "UNAVAILABLE", "{refused}");
    assert_eq!(extensions["retryAfterSeconds"], 30, "{refused}");
}

/// The instrument is the method's, as anywhere: above the policy its
/// `err(Debug)` records each attempt's error, below it the run's — here
/// the open breaker's refusal, the first field having opened it.
#[tokio::test(start_paused = true)]
async fn an_instruments_err_on_an_operation_records_the_fields_error_as_debug() {
    let app = TestApp::for_module::<DecoratedGraphqlModule>()
        .await
        .expect("a decorated resolver boots");
    let logs = LogCapture::install();
    for field in ["tracedAbove", "tracedBelow"] {
        let answered = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": format!("{{ {field} }}") }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize::<Value>();
        assert!(answered["errors"][0].is_object(), "{answered}");
    }
    let recorded: Vec<_> = logs
        .events_at(tracing::Level::ERROR)
        .into_iter()
        .filter(|event| event.target == module_path!())
        .filter_map(|event| event.field("error"))
        .collect();
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    assert_eq!(recorded[0], "UpstreamDown", "an attempt's: {recorded:?}");
    assert!(
        recorded[1].starts_with("Refused { refusal: Open {"),
        "the run's refusal: {recorded:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_subscription_answers_its_refusal_unavailable_with_the_breakers_wait() {
    let app = TestApp::for_module::<DecoratedGraphqlModule>()
        .await
        .expect("a decorated subscription boots");
    let mut socket = app.graphql_socket().open();
    socket.connect().await;

    socket.subscribe("failed", "subscription { movements }");
    let failed = socket
        .next_item("failed")
        .await
        .expect("the opening answers");
    assert_eq!(
        failed["errors"][0]["extensions"]["code"], "INTERNAL",
        "{failed}"
    );

    socket.subscribe("refused", "subscription { movements }");
    let refused = socket
        .next_item("refused")
        .await
        .expect("the opening answers");
    let extensions = &refused["errors"][0]["extensions"];
    assert_eq!(extensions["code"], "UNAVAILABLE", "{refused}");
    assert_eq!(extensions["retryAfterSeconds"], 30, "{refused}");
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(
        attempts.made(),
        1,
        "sent once, and the opening the breaker refused never"
    );
}

/// A parcel, which a field resolver extends and an entity resolves by its
/// number.
#[derive(async_graphql::SimpleObject)]
#[graphql(complex)]
struct Parcel {
    number: String,
}

#[resolver]
struct RetriedResolver {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

/// `#[operations]` reads a field's `Result` by its spelling, so each
/// decorated role writes it out.
#[operations]
impl RetriedResolver {
    #[query]
    #[public]
    async fn parcel(&self) -> Parcel {
        Parcel {
            number: "P1".to_owned(),
        }
    }

    #[query]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn tracked(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok("in transit".to_owned())
    }

    #[mutation]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn reroute(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok("rerouted".to_owned())
    }

    #[field_resolver]
    #[resilient(upstream, idempotent)]
    async fn depot(&self, parent: &Parcel) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(format!("{} waits in Lyon", parent.number))
    }

    #[entity]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn find_parcel_by_number(
        &self,
        number: String,
    ) -> Result<Parcel, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(Parcel {
            number: number.clone(),
        })
    }

    /// The parcels arriving, the box unsized where the method answers it.
    #[subscription]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn arrivals(&self) -> Result<BoxStream<'static, i32>, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(Box::pin(stream::iter([7])))
    }
}

#[module(
    imports = [
        GraphqlModule::for_root(GraphqlConfig {
            federation: true,
            federation_sdl: nest_rs_config::Surface::Open,
            ..GraphqlConfig::default()
        }),
        ResilienceModule::for_root(retrying_once()),
    ],
    providers = [RetriedResolver, Attempts],
)]
struct RetriedGraphqlModule;

/// What the schema answers `body` with, and how many attempts the one
/// decorated method it reached made.
async fn retried(body: Value) -> (Value, u32) {
    let app = TestApp::for_module::<RetriedGraphqlModule>()
        .await
        .expect("a resolver decorating every role boots");
    let answer = app
        .http()
        .post("/graphql")
        .body_json(&body)
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize::<Value>();
    let attempts = app.container().get::<Attempts>().expect("provided");
    (answer, attempts.made())
}

#[tokio::test(start_paused = true)]
async fn a_query_failing_once_answers_on_its_retry_within_its_one_operation() {
    let logs = LogCapture::install();
    let (answer, attempts) = retried(json!({ "query": "{ tracked }" })).await;
    assert_eq!(answer["data"]["tracked"], "in transit", "{answer}");
    assert_eq!(answer.get("errors"), None, "{answer}");
    assert_eq!(attempts, 2, "the failed attempt and its retry");
    retried_once(&logs, "RetriedResolver", "tracked");
    let operation = logs.expect_one(OPERATIONS, nest_rs_graphql::unit::OPERATION.name());
    assert_eq!(
        operation.field("outcome").as_deref(),
        Some(OK),
        "{operation:?}"
    );
    logs.expect_one(OPERATIONS, nest_rs_http::unit::REQUEST.name());
}

#[tokio::test(start_paused = true)]
async fn a_mutation_failing_once_answers_on_its_retry() {
    let (answer, attempts) = retried(json!({ "query": "mutation { reroute }" })).await;
    assert_eq!(answer["data"]["reroute"], "rerouted", "{answer}");
    assert_eq!(attempts, 2, "the failed attempt and its retry");
}

#[tokio::test(start_paused = true)]
async fn a_field_resolver_failing_once_answers_on_its_retry() {
    let (answer, attempts) = retried(json!({ "query": "{ parcel { depot } }" })).await;
    assert_eq!(
        answer["data"]["parcel"]["depot"], "P1 waits in Lyon",
        "{answer}"
    );
    assert_eq!(attempts, 2, "the failed attempt and its retry");
}

#[tokio::test(start_paused = true)]
async fn an_entity_failing_once_answers_its_reference_on_its_retry() {
    let (answer, attempts) = retried(json!({
        "query": "query($reps: [_Any!]!) { \
                  _entities(representations: $reps) { ... on Parcel { number } } }",
        "variables": { "reps": [{ "__typename": "Parcel", "number": "P7" }] },
    }))
    .await;
    assert_eq!(answer["data"]["_entities"][0]["number"], "P7", "{answer}");
    assert_eq!(attempts, 2, "the failed attempt and its retry");
}

#[tokio::test(start_paused = true)]
async fn a_subscription_failing_once_answers_on_its_retry() {
    let app = TestApp::for_module::<RetriedGraphqlModule>()
        .await
        .expect("a resolver decorating every role boots");
    let mut socket = app.graphql_socket().open();
    socket.connect().await;

    socket.subscribe("retried", "subscription { arrivals }");
    let item = socket
        .next_item("retried")
        .await
        .expect("the opening answers");
    assert_eq!(item["data"]["arrivals"], 7, "{item}");
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
}
