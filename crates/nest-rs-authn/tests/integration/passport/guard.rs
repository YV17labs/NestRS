//! Covers `src/passport/guard.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authn::{AuthError, AuthnGuard, Strategy};
use nest_rs_guards::{Denial, Guard};
use nest_rs_testing::LogCapture;
use poem::Request;

struct AuthenticateAs(&'static str);

#[async_trait]
impl Strategy for AuthenticateAs {
    type Principal = &'static str;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Ok(self.0)
    }
}

struct FailWith;

#[async_trait]
impl Strategy for FailWith {
    type Principal = ();

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

struct RejectWith(fn() -> AuthError);

#[async_trait]
impl Strategy for RejectWith {
    type Principal = ();

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err((self.0)())
    }
}

/// A request carrying the `#[public]` marker the route macro attaches.
fn public_request() -> Request {
    let mut req = crate::request(&[]);
    req.extensions_mut().insert(nest_rs_http::Public);
    req
}

#[tokio::test]
async fn attaches_principal_on_success() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateAs("alice")));
    let mut req = crate::request(&[]);

    guard.check_http(&mut req).await.expect("guard passes");
    assert_eq!(req.extensions().get::<&'static str>(), Some(&"alice"));
}

#[tokio::test]
async fn strategy_error_denies_as_unauthorized() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    let mut req = crate::request(&[]);

    let denial = guard.check_http(&mut req).await.expect_err("auth failed");
    assert!(matches!(denial, Denial::Unauthorized { .. }));
    assert!(req.extensions().get::<&'static str>().is_none());
}

#[tokio::test]
async fn public_route_admits_an_anonymous_caller() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    guard
        .check_http(&mut public_request())
        .await
        .expect("no credential on a public route is not a failure");
}

#[tokio::test]
async fn public_route_admits_a_rejected_credential_as_anonymous() {
    // The posture `#[public]` promises: a forged token does not turn a public
    // route into a 401 — it is logged and the request continues anonymously.
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| AuthError::InvalidSignature)));
    guard
        .check_http(&mut public_request())
        .await
        .expect("a rejected credential still leaves the public route reachable");

    // "and it is logged" is half the promise, and the half nothing read. The
    // request succeeds, so this event is the *only* trace a forged token
    // probing a public endpoint leaves — which is why it is `warn` and not the
    // `debug` its no-credential sibling gets.
    let event = logs.expect_one(
        "nest_rs::authn",
        "rejected credential on a public route — continuing as anonymous",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("reason").as_deref(), Some("invalid_signature"));
    assert!(
        event.field("strategy").is_some(),
        "the event names the strategy that rejected it, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn unreachable_store_fails_closed_even_on_a_public_route() {
    // The credential was never evaluated, so serving the caller as anonymous
    // would silently downgrade every authenticated session during an outage.
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| {
        AuthError::Unavailable("store unreachable".into())
    })));

    let denial = guard
        .check_http(&mut public_request())
        .await
        .expect_err("an unevaluated credential must not pass as anonymous");
    assert!(matches!(denial, Denial::Internal { .. }), "{denial:?}");

    // The client message is deliberately opaque, so the outage is only ever
    // readable here — and an outage that logs a bare line is the one an
    // operator cannot correlate to a strategy.
    let event = logs.expect_one(
        "nest_rs::authn",
        "authentication unavailable — identity store unreachable",
    );
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("store unreachable")),
        "the event carries the underlying failure, got {:?}",
        event.fields,
    );
    assert!(event.field("strategy").is_some(), "{:?}", event.fields);
}

/// Authentication is the one moment anybody learns who is calling, so it is the
/// one place the ambient identity can be set. Everything downstream — a service
/// stamping `created_by`, the queue producer sealing a job, an audit row — reads
/// it back through `current_actor_id()` rather than having the handler thread it
/// down, which is what makes the answer available at call sites the handler
/// never passes through.
#[tokio::test]
async fn a_successful_check_publishes_the_actor_into_the_ambient_context() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateAs("ada")));
    let correlation = nest_rs_core::Correlation::mint();

    let seen = nest_rs_core::with_request_scope(None, correlation, async {
        let mut req = Request::default();
        guard
            .check_http(&mut req)
            .await
            .expect("the strategy authenticates");
        nest_rs_core::current_actor_id()
    })
    .await;

    assert_eq!(
        seen.as_deref(),
        Some("ada"),
        "the actor must be readable below the guard without being threaded through",
    );
}

/// An anonymous caller is reported as **absent**, never as a sentinel string: a
/// query counting anonymous traffic must not also count an actor genuinely named
/// `""` or `"anonymous"`.
#[tokio::test]
async fn an_unauthenticated_caller_has_no_ambient_actor() {
    let correlation = nest_rs_core::Correlation::mint();

    let seen = nest_rs_core::with_request_scope(None, correlation, async {
        // No guard ran at all — the shape of every request before authentication
        // and of every `#[public]` route reached without a credential.
        nest_rs_core::current_actor_id()
    })
    .await;

    assert_eq!(seen, None);
}
