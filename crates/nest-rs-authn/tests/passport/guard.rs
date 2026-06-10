//! Covers `src/passport/guard.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authn::{AuthError, AuthGuard, Strategy};
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

#[tokio::test]
async fn attaches_principal_on_success() {
    let guard = AuthGuard::new(Arc::new(AuthenticateAs("alice")));
    let mut req = crate::common::request(&[]);

    guard.check_http(&mut req).await.expect("guard passes");
    assert_eq!(req.extensions().get::<&'static str>(), Some(&"alice"));
}

#[tokio::test]
async fn strategy_error_denies_as_unauthorized() {
    let guard = AuthGuard::new(Arc::new(FailWith));
    let mut req = crate::common::request(&[]);

    let denial = guard.check_http(&mut req).await.expect_err("auth failed");
    assert!(matches!(denial, Denial::Unauthorized { .. }));
    assert!(req.extensions().get::<&'static str>().is_none());
}
