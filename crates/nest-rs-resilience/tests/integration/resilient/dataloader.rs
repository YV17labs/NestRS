//! A decorated batch of a `#[dataloader]` host: the loader's error is the
//! method's `ResilienceError`, which async-graphql bounds `Clone` and hands
//! each key of a batch a clone of, so a refusal answers every field the batch
//! served.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_graphql::async_graphql::dataloader::DataLoader;
use nest_rs_graphql::{GraphqlModule, dataloader, operations, resolver};
use nest_rs_resilience::{
    ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};

use super::Attempts;

/// What the partners' API answers when it is down: a failure no client reads,
/// which a loader's error clones.
#[derive(Clone, Debug, thiserror::Error)]
#[error("the partners' API answered 502")]
#[expect(
    unreachable_pub,
    reason = "the loader `#[dataloader]` emits is `pub`, and so is the error its `Loader` impl names"
)]
pub struct PartnersDown;

impl nest_rs_core::ToProblem for PartnersDown {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        None
    }
}

#[injectable]
struct Partners {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[dataloader]
impl Partners {
    /// The partners' names, one batch per request, its first attempt failing.
    #[resilient(upstream, idempotent)]
    async fn names(
        &self,
        ids: &[u32],
    ) -> Result<HashMap<u32, String>, ResilienceError<PartnersDown>> {
        if self.attempts.first() {
            return Err(PartnersDown);
        }
        Ok(ids
            .iter()
            .map(|id| (*id, format!("partner {id}")))
            .collect())
    }
}

#[resolver]
struct PartnersResolver;

#[operations]
impl PartnersResolver {
    #[query]
    #[public]
    async fn partner(
        &self,
        id: u32,
        names: &DataLoader<PartnersNames>,
    ) -> Result<Option<String>, ResilienceError<PartnersDown>> {
        names.load_one(id).await
    }
}

#[module(
    imports = [
        GraphqlModule,
        ResilienceModule::for_root(ResilienceConfig {
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            ..ResilienceConfig::default()
        }),
    ],
    providers = [Partners, PartnersResolver, Attempts],
)]
struct PartnersModule;

/// Two fields asking one batch: it fails once and answers both on its retry,
/// then, its breaker open, each field answers the refusal with its wait.
#[tokio::test(start_paused = true)]
async fn a_batch_answers_each_key_on_its_retry_and_its_refusal_each_field() {
    let app = TestApp::for_module::<PartnersModule>()
        .await
        .expect("a decorated batch boots");
    let query = || async {
        app.http()
            .post("/graphql")
            .body_json(&json!({ "query": "{ first: partner(id: 1) second: partner(id: 2) }" }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize::<Value>()
    };

    let answered = query().await;
    assert_eq!(
        answered["data"],
        json!({ "first": "partner 1", "second": "partner 2" }),
        "{answered}"
    );
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "one batch: its failure and its retry");

    let upstream = app
        .container()
        .get::<ResiliencePolicy>()
        .expect("the module built the policy");
    upstream
        .open_breaker(Duration::from_secs(30))
        .expect("the policy has a breaker");
    let most = crate::breaker_wait(&upstream);
    let refused = query().await;
    let least = crate::breaker_wait(&upstream);
    assert_eq!(
        refused["data"],
        json!({ "first": null, "second": null }),
        "{refused}"
    );
    let errors = refused["errors"].as_array().expect("each field's error");
    assert_eq!(errors.len(), 2, "{refused}");
    for error in errors {
        assert_eq!(error["extensions"]["code"], "UNAVAILABLE", "{refused}");
        let told = error["extensions"]["retryAfterSeconds"].as_u64();
        assert!(
            told.is_some_and(|told| (least..=most).contains(&told)),
            "the breaker's wait as it refused, {least} to {most} s: {refused}"
        );
    }
    assert_eq!(attempts.made(), 2, "the open breaker sent nothing");
}
