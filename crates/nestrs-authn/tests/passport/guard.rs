//! Covers `src/passport/guard.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use nestrs_authn::{AuthError, AuthGuard, Outcome, Strategy};
use nestrs_http::Guard;
use poem::http::StatusCode;
use poem::{Body, Request, Response};

struct AuthenticateAs(&'static str);

#[async_trait]
impl Strategy for AuthenticateAs {
    type Principal = &'static str;

    async fn authenticate(
        &self,
        _req: &mut Request,
    ) -> Result<Outcome<Self::Principal>, AuthError> {
        Ok(Outcome::Authenticated(self.0))
    }
}

struct IssueChallenge;

#[async_trait]
impl Strategy for IssueChallenge {
    type Principal = ();

    async fn authenticate(
        &self,
        _req: &mut Request,
    ) -> Result<Outcome<Self::Principal>, AuthError> {
        Ok(Outcome::Challenge(
            Response::builder()
                .status(StatusCode::FOUND)
                .body(Body::empty()),
        ))
    }
}

struct FailWith;

#[async_trait]
impl Strategy for FailWith {
    type Principal = ();

    async fn authenticate(
        &self,
        _req: &mut Request,
    ) -> Result<Outcome<Self::Principal>, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

#[tokio::test]
async fn attaches_principal_on_success() {
    let guard = AuthGuard::new(Arc::new(AuthenticateAs("alice")));
    let mut req = crate::common::request(&[]);

    guard.check(&mut req).await.expect("guard passes");
    assert_eq!(req.extensions().get::<&'static str>(), Some(&"alice"));
}

#[tokio::test]
async fn returns_challenge_response_without_attaching_principal() {
    let guard = AuthGuard::new(Arc::new(IssueChallenge));
    let mut req = crate::common::request(&[]);

    let response = guard
        .check(&mut req)
        .await
        .expect_err("challenge stops the chain");
    assert_eq!(response.status(), StatusCode::FOUND);
    assert!(req.extensions().get::<&'static str>().is_none());
}

#[tokio::test]
async fn maps_strategy_error_to_unauthorized_response() {
    let guard = AuthGuard::new(Arc::new(FailWith));
    let mut req = crate::common::request(&[]);

    let response = guard.check(&mut req).await.expect_err("auth failed");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
