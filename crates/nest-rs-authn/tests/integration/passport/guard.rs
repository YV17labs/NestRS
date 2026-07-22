//! Covers `src/passport/guard.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authn::{AuthError, AuthnGuard, Strategy};
use nest_rs_guards::{Denial, Guard};
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
    let mut req = crate::common::request(&[]);
    req.extensions_mut().insert(nest_rs_core::Public);
    req
}

#[tokio::test]
async fn attaches_principal_on_success() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateAs("alice")));
    let mut req = crate::common::request(&[]);

    guard.check_http(&mut req).await.expect("guard passes");
    assert_eq!(req.extensions().get::<&'static str>(), Some(&"alice"));
}

#[tokio::test]
async fn strategy_error_denies_as_unauthorized() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    let mut req = crate::common::request(&[]);

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
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| AuthError::InvalidSignature)));
    guard
        .check_http(&mut public_request())
        .await
        .expect("a rejected credential still leaves the public route reachable");
}

#[tokio::test]
async fn unreachable_store_fails_closed_even_on_a_public_route() {
    // The credential was never evaluated, so serving the caller as anonymous
    // would silently downgrade every authenticated session during an outage.
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| {
        AuthError::Unavailable("store unreachable".into())
    })));

    let denial = guard
        .check_http(&mut public_request())
        .await
        .expect_err("an unevaluated credential must not pass as anonymous");
    assert!(matches!(denial, Denial::Internal { .. }), "{denial:?}");
}
