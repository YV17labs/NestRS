//! [`ResourceChallenge`] — stamps the OAuth challenge onto every refusal the
//! process emits: the RFC 9728 discovery pointer on a `401`, and the RFC 6750
//! `insufficient_scope` challenge on a `403` whose token was merely too narrow.
//!
//! Both halves of the client's flow therefore have one writer. A client that
//! holds no token learns where to get one; a client whose token is too narrow
//! learns which scope to ask for — and neither answer can drift from the
//! metadata document, because the same frozen value produces all three.
//!
//! **Why the transport edge and not `AuthError`.** A `401` leaves this process
//! from more places than the framework's own error type: a guard denial rendered
//! as `problem+json`, the MCP endpoint's deny-all fallback, rmcp's own transport
//! refusals, a hand-written handler. The spec's MUST is about the response, not
//! about who wrote it — so the challenge is attached where every response
//! converges. One seam covers three of the four transports:
//!
//! - **HTTP** — the ordinary `401`.
//! - **WS** — the upgrade is an HTTP `GET` carrying the real guards, so its
//!   refusal is an ordinary `401` too.
//! - **MCP** — `EdgePosture::Exempt` skips the guard chain, not this band; the
//!   in-band operation guard writes a real `401`.
//! - **GraphQL** — the odd one out, and deliberately: `/graphql` answers an
//!   unauthenticated operation with `200 OK` + an `UNAUTHENTICATED` error frame,
//!   so there is no `401` to enrich. A client discovers the authorization server
//!   through the well-known document instead, which the spec offers as the
//!   equal alternative to the header.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_guards::RequiredScopes;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::{HeaderValue, StatusCode, header};
use poem::{Request, Response, Result};

use crate::resource::metadata::ProtectedResourceMetadata;

/// Opt a `401` out of the `Bearer` challenge, by inserting it into the
/// response's extensions.
///
/// One shipped use: a password-login rejection. `POST /auth/login` is not a
/// protected resource — telling that caller to go fetch an OAuth token for a
/// *different* server is misdirection, not discovery. An app whose own `401`
/// means the same thing marks it the same way.
#[derive(Clone, Copy, Debug)]
pub struct NoBearerChallenge;

/// Infra interceptor brought by
/// [`ProtectedResourceModule`](crate::ProtectedResourceModule). Auto-mounted at
/// the transport edge; never listed as a provider.
#[interceptor]
pub(crate) struct ResourceChallenge {
    #[inject]
    metadata: Arc<ProtectedResourceMetadata>,
}

impl Layer for ResourceChallenge {}

#[async_trait]
impl Interceptor for ResourceChallenge {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        match next.run(req).await {
            Ok(res) => Ok(self.stamp(res)),
            // A `401` that is still an `Err` here (a handler's `?`, an
            // extractor rejection) renders to the same bytes as one that was
            // already `Ok` — it must carry the same challenge.
            Err(err) => {
                let res = self.stamp(err.into_response());
                Err(poem::Error::from_response(res))
            }
        }
    }
}

impl ResourceChallenge {
    fn stamp(&self, mut res: Response) -> Response {
        if res.status() == StatusCode::FORBIDDEN {
            return self.stamp_insufficient_scope(res);
        }
        if res.status() != StatusCode::UNAUTHORIZED {
            return res;
        }
        if res.extensions().get::<NoBearerChallenge>().is_some() {
            return res;
        }
        // Never overwrite a challenge that already carries the pointer, and
        // never touch a non-`Bearer` scheme (a `Basic` or `DPoP` challenge is
        // someone else's contract). A bare `Bearer` — what the framework used
        // to emit — is exactly what this replaces.
        if let Some(existing) = res.headers().get(header::WWW_AUTHENTICATE) {
            let Ok(existing) = existing.to_str() else {
                return res;
            };
            let is_plain_bearer = existing
                .trim_start()
                .get(.."bearer".len())
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer"));
            if !is_plain_bearer || existing.contains("resource_metadata") {
                return res;
            }
        }
        match HeaderValue::from_str(self.metadata.challenge()) {
            Ok(value) => {
                res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
            }
            // Unreachable: every component was character-checked at boot.
            // Logged rather than swallowed — a 401 without the pointer is a
            // conformance failure, and silence would hide it.
            Err(error) => tracing::error!(
                target: "nest_rs::authn",
                %error,
                challenge = self.metadata.challenge(),
                "protected resource challenge is not a valid header value",
            ),
        }
        res
    }

    /// The `403` half of RFC 6750 §3.1: a token that verified but is too
    /// narrow. Only a response carrying [`RequiredScopes`] qualifies — an
    /// ordinary `403` (the caller may not do this at all, whatever token they
    /// hold) gets no challenge, because telling that caller to go widen their
    /// token is a instruction that cannot succeed.
    fn stamp_insufficient_scope(&self, mut res: Response) -> Response {
        let Some(required) = res.extensions().get::<RequiredScopes>() else {
            return res;
        };
        if required.is_empty() {
            return res;
        }
        let required = required.as_slice().to_vec();

        // A scope the client is told to request but the document never
        // advertises is a dead end: the client asks the authorization server
        // for something discovery never named. This is the one place both
        // halves are known, so it is where the drift is caught.
        let advertised = self.metadata.scopes_supported();
        if !advertised.is_empty() {
            let unadvertised: Vec<&str> = required
                .iter()
                .filter(|scope| !advertised.contains(scope))
                .map(String::as_str)
                .collect();
            if !unadvertised.is_empty() {
                tracing::warn!(
                    target: "nest_rs::authn",
                    scopes = ?unadvertised,
                    reason = "scope_not_advertised",
                    "denied for a scope this resource does not advertise — a client following \
                     the metadata document cannot request it",
                );
            }
        }

        let challenge = self.metadata.insufficient_scope_challenge(&required);
        match HeaderValue::from_str(&challenge) {
            Ok(value) => {
                res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
            }
            // The resource and metadata URL were checked at boot, so only a
            // scope carrying a quote or control character reaches here — which
            // the config refuses too. Logged rather than swallowed.
            Err(error) => tracing::error!(
                target: "nest_rs::authn",
                %error,
                challenge,
                "insufficient-scope challenge is not a valid header value",
            ),
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge_for(res: Response) -> Option<String> {
        challenge_advertising(res, Vec::new())
    }

    fn challenge_advertising(res: Response, scopes_supported: Vec<String>) -> Option<String> {
        let metadata = ProtectedResourceMetadata::new(
            "https://api.example.com".into(),
            vec!["https://auth.example.com".into()],
            scopes_supported,
            vec!["header".into()],
            None,
            None,
        );
        let stamped = ResourceChallenge {
            metadata: Arc::new(metadata),
        }
        .stamp(res);
        stamped
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .map(|v| v.to_str().expect("ascii").to_owned())
    }

    fn status(code: StatusCode) -> Response {
        Response::builder().status(code).finish()
    }

    #[test]
    fn a_bare_bearer_challenge_is_upgraded_to_carry_the_pointer() {
        let mut res = status(StatusCode::UNAUTHORIZED);
        res.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        assert_eq!(
            challenge_for(res).as_deref(),
            Some(
                "Bearer resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\""
            ),
        );
    }

    #[test]
    fn a_401_with_no_challenge_at_all_gets_one() {
        // The guard-denial path renders `problem+json` and sets no challenge;
        // it is the most common 401 in the framework.
        assert!(challenge_for(status(StatusCode::UNAUTHORIZED)).is_some());
    }

    #[test]
    fn a_non_401_is_left_alone() {
        assert!(challenge_for(status(StatusCode::FORBIDDEN)).is_none());
        assert!(challenge_for(status(StatusCode::OK)).is_none());
    }

    #[test]
    fn a_scope_denial_names_the_error_and_the_missing_scope() {
        // The step-up half of the flow: the token verified, so this is not a
        // `401`, but the client can still fix it — by asking the authorization
        // server for `posts:write`.
        let mut res = status(StatusCode::FORBIDDEN);
        res.extensions_mut()
            .insert(RequiredScopes::new(["posts:write"]));
        let challenge = challenge_for(res).expect("a scope denial carries a challenge");

        assert!(
            challenge.contains("error=\"insufficient_scope\""),
            "{challenge}"
        );
        assert!(challenge.contains("scope=\"posts:write\""), "{challenge}");
        assert!(
            challenge.contains(
                "resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\""
            ),
            "the step-up challenge points at the same document as the 401: {challenge}",
        );
    }

    #[test]
    fn an_ordinary_403_gets_no_challenge() {
        // "You may not do this at all" is not fixable by a wider token, and
        // telling that caller to go get one sends them somewhere that cannot
        // help. Only a response carrying `RequiredScopes` is a scope denial.
        assert!(challenge_for(status(StatusCode::FORBIDDEN)).is_none());

        let mut empty = status(StatusCode::FORBIDDEN);
        empty
            .extensions_mut()
            .insert(RequiredScopes::new(Vec::<String>::new()));
        assert!(
            challenge_for(empty).is_none(),
            "an empty requirement would render as `scope=\"\"`",
        );
    }

    #[test]
    fn a_scope_denial_does_not_disturb_the_401_path() {
        // The two branches are keyed on status, so the 401 rules above (foreign
        // scheme, richer challenge, opt-out) cannot be reached by a 403 and
        // vice versa.
        let mut res = status(StatusCode::UNAUTHORIZED);
        res.extensions_mut()
            .insert(RequiredScopes::new(["posts:write"]));
        let challenge = challenge_for(res).expect("still a 401 challenge");
        assert!(
            !challenge.contains("insufficient_scope"),
            "a 401 is `no token`, never `too narrow a token`: {challenge}",
        );
    }

    #[test]
    fn an_advertised_scope_and_an_unadvertised_one_both_reach_the_client() {
        // The `warn` for the unadvertised case is the operator's signal, not
        // the client's: the challenge is still the client's best next step, so
        // it is emitted either way.
        let mut res = status(StatusCode::FORBIDDEN);
        res.extensions_mut()
            .insert(RequiredScopes::new(["posts:admin"]));
        let challenge = challenge_advertising(res, vec!["posts:read".into()])
            .expect("the challenge is emitted regardless");
        assert!(challenge.contains("scope=\"posts:admin\""), "{challenge}");
    }

    #[test]
    fn a_foreign_scheme_is_never_replaced() {
        let mut res = status(StatusCode::UNAUTHORIZED);
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"admin\""),
        );
        assert_eq!(challenge_for(res).as_deref(), Some("Basic realm=\"admin\""));
    }

    #[test]
    fn a_richer_bearer_challenge_is_left_intact() {
        // A handler that already spelled out `resource_metadata` (a step-up
        // challenge, a different resource) knows more than this interceptor.
        let mut res = status(StatusCode::UNAUTHORIZED);
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static(
                "Bearer error=\"invalid_token\", resource_metadata=\"https://other.example/x\"",
            ),
        );
        assert!(
            challenge_for(res)
                .expect("kept")
                .contains("https://other.example/x"),
        );
    }

    #[test]
    fn the_opt_out_marker_suppresses_the_challenge() {
        let mut res = status(StatusCode::UNAUTHORIZED);
        res.extensions_mut().insert(NoBearerChallenge);
        assert!(challenge_for(res).is_none());
    }
}
