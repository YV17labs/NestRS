//! Covers `src/resilience.rs`: an idempotent request a failing peer answers is
//! sent again on the policy's retries, one that is not is sent once, and an
//! open breaker refuses without reaching the peer.

use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_core::Backoff;
use nest_rs_reqwest::ResilienceMiddleware;
use nest_rs_resilience::{CircuitBreaker, Policy, Retry};
use reqwest_middleware::ClientWithMiddleware;

use crate::{AUTHORITY, Behaviour, Peer};

fn through(policy: Policy) -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .build()
        .expect("a client");
    reqwest_middleware::ClientBuilder::new(client)
        .with(ResilienceMiddleware::new(policy))
        .build()
}

fn retrying(name: &str) -> Policy {
    Policy::builder(name)
        .timeout(Duration::from_secs(10))
        .retry(Retry::new(
            NonZeroU32::new(3).expect("non-zero"),
            Backoff::new(Duration::from_millis(1), Duration::from_millis(5)),
        ))
        .build()
        .expect("a policy")
}

#[tokio::test]
async fn an_idempotent_request_a_failing_peer_answers_is_sent_again() {
    let peer = Peer::serving(Behaviour::Answer(503)).await;
    let policy = retrying("peer-get");
    let response = through(policy.clone())
        .get(peer.url("/"))
        .send()
        .await
        .expect("the last answer");
    assert_eq!(
        response.status(),
        503,
        "the last attempt's answer, as it is"
    );
    assert_eq!(peer.requests(), 3);
    assert_eq!(policy.stats().retried, 2);
}

#[tokio::test]
async fn a_request_that_is_not_idempotent_is_sent_once() {
    let peer = Peer::serving(Behaviour::Answer(503)).await;
    let response = through(retrying("peer-post"))
        .post(peer.url("/"))
        .body("charge")
        .send()
        .await
        .expect("an answer");
    assert_eq!(response.status(), 503);
    assert_eq!(peer.requests(), 1);
}

#[tokio::test]
async fn a_request_carrying_an_idempotency_key_is_retriable() {
    let peer = Peer::serving(Behaviour::Answer(502)).await;
    through(retrying("peer-keyed"))
        .post(peer.url("/"))
        .header("Idempotency-Key", "order-7")
        .body("charge")
        .send()
        .await
        .expect("an answer");
    assert_eq!(peer.requests(), 3);
}

#[tokio::test]
async fn a_client_error_is_the_peer_answering() {
    let peer = Peer::serving(Behaviour::Answer(404)).await;
    let response = through(retrying("peer-404"))
        .get(peer.url("/"))
        .send()
        .await
        .expect("an answer");
    assert_eq!(response.status(), 404);
    assert_eq!(peer.requests(), 1);
}

#[tokio::test]
async fn an_open_breaker_refuses_without_reaching_the_peer() {
    let peer = Peer::serving(Behaviour::Answer(500)).await;
    let policy = Policy::builder("peer-breaker")
        .circuit_breaker(CircuitBreaker::new(
            NonZeroU32::MIN,
            100,
            Duration::from_secs(60),
        ))
        .build()
        .expect("a policy");
    let client = through(policy);
    client.get(peer.url("/")).send().await.expect("an answer");
    let refused = client.get(peer.url("/")).send().await.expect_err("refused");
    let reqwest_middleware::Error::Middleware(refused) = refused else {
        panic!("a middleware error, got {refused:?}");
    };
    assert!(
        refused.chain().any(|link| link
            .to_string()
            .contains("circuit of policy `peer-breaker` is open")),
        "{refused:#}"
    );
    assert_eq!(peer.requests(), 1);
}
