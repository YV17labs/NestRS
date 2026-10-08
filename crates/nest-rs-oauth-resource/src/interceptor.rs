//! [`ResourceChallenge`] — stamps the OAuth challenge onto every refusal the
//! process emits: the RFC 9728 discovery pointer on a `401`, and the RFC 6750
//! `insufficient_scope` challenge on a `403` whose token was merely too narrow.
//!
//! It sits at the transport edge, where every `401` converges — a guard denial,
//! the MCP fallback, rmcp, a handler. GraphQL answers `200` + `UNAUTHENTICATED`,
//! so a GraphQL client discovers through the well-known document instead.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_guards::{NoBearerChallenge, RequiredScopes};
use nest_rs_http::challenge::BEARER;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::{HeaderValue, StatusCode, header};
use poem::{Request, Response, Result};

use crate::metadata::ProtectedResourceMetadata;

/// Infra interceptor brought by
/// [`OAuthResourceModule`](crate::OAuthResourceModule). Auto-mounted at
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
            // A `401` still an `Err` here renders to the same bytes, so it
            // carries the same challenge.
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
        // Never touch a non-`Bearer` scheme or a challenge already carrying the
        // pointer; otherwise merge the pointer in, keeping RFC 6750 §3.1's `error`.
        let merged = match res.headers().get(header::WWW_AUTHENTICATE) {
            // Nothing written yet (the guard-denial path): the full challenge,
            // with the RFC 6750 §3.1 code the guard recorded on the request.
            None => Cow::Borrowed(self.metadata.challenge()),
            Some(existing) => {
                let Ok(existing) = existing.to_str() else {
                    return res;
                };
                let existing = existing.trim();
                let is_bearer = existing
                    .get(..BEARER.len())
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case(BEARER));
                if !is_bearer || existing.contains("resource_metadata") {
                    return res;
                }
                let params = existing[BEARER.len()..].trim_start().trim_end_matches(',');
                if params.is_empty() {
                    Cow::Borrowed(self.metadata.challenge())
                } else {
                    // `challenge()` is `Bearer <params>`: this response's params go first.
                    let pointer = self.metadata.challenge()[BEARER.len()..].trim_start();
                    Cow::Owned(format!("{BEARER} {params}, {pointer}"))
                }
            }
        };
        match HeaderValue::from_str(&merged) {
            Ok(value) => {
                res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
            }
            // Unreachable: every component was character-checked at boot.
            Err(error) => tracing::error!(
                target: crate::TARGET,
                %error,
                challenge = %merged,
                "protected resource challenge is not a valid header value",
            ),
        }
        res
    }

    /// The `403` half of RFC 6750 §3.1: a token that verified but is too narrow.
    /// Only a response carrying [`RequiredScopes`] qualifies; an ordinary `403`
    /// cannot be fixed by a wider token, so it gets no challenge.
    fn stamp_insufficient_scope(&self, mut res: Response) -> Response {
        let Some(required) = res.extensions().get::<RequiredScopes>() else {
            return res;
        };
        if required.is_empty() {
            return res;
        }
        let required = required.as_slice().to_vec();

        // A required scope the document never advertises is a dead end for the
        // client; this is the one place both halves are known.
        let advertised = self.metadata.scopes_supported();
        if !advertised.is_empty() {
            let unadvertised: Vec<&str> = required
                .iter()
                .filter(|scope| !advertised.contains(scope))
                .map(String::as_str)
                .collect();
            if !unadvertised.is_empty() {
                tracing::warn!(
                    target: crate::TARGET,
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
            // Unreachable: the config refuses a scope with a quote or control character.
            Err(error) => tracing::error!(
                target: crate::TARGET,
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
            Default::default(),
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
        assert!(challenge_for(status(StatusCode::UNAUTHORIZED)).is_some());
    }

    #[test]
    fn a_non_401_is_left_alone() {
        assert!(challenge_for(status(StatusCode::FORBIDDEN)).is_none());
        assert!(challenge_for(status(StatusCode::OK)).is_none());
    }

    #[test]
    fn a_scope_denial_names_the_error_and_the_missing_scope() {
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
