//! [`ResourceChallenge`] — stamps the OAuth challenge onto every refusal an
//! HTTP unit answers: the RFC 9728 discovery pointer on a `401`, and the RFC
//! 6750 `insufficient_scope` challenge on a `403` whose token was merely too
//! narrow.
//!
//! It runs at the unit stage of every HTTP unit, where each `401` converges — a
//! guard's denial, the MCP fallback, rmcp, a handler, what the router answers.
//! GraphQL answers `200` + `UNAUTHENTICATED`, so a GraphQL client discovers
//! through the well-known document instead.

use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_core::operation_log::Unit;
use nest_rs_core::{
    Edge, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, interceptor,
};
use nest_rs_guards::RequiredScopes;
use nest_rs_http::challenge::{BEARER, INSUFFICIENT_SCOPE, NoBearerChallenge};
use nest_rs_http::{HeaderValue, HttpView, Response, StatusCode, header};

use crate::config::is_scope_token;
use crate::metadata::ProtectedResourceMetadata;

/// Infra interceptor brought by
/// [`OAuthResourceModule`](crate::OAuthResourceModule), at the unit stage of
/// every HTTP unit, around its guards.
#[interceptor(stage = unit)]
pub(crate) struct ResourceChallenge {
    #[inject]
    metadata: Arc<ProtectedResourceMetadata>,
}

impl Layer for ResourceChallenge {
    fn serves(&self, unit: Unit) -> bool {
        unit.edge() == Some(Edge::Http)
    }
}

#[async_trait]
impl Interceptor for ResourceChallenge {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        if let Some(response) = HttpView::of(cx)?.response_mut() {
            self.stamp(response);
        }
        outcome
    }
}

impl ResourceChallenge {
    fn stamp(&self, res: &mut Response) {
        if res.status() == StatusCode::FORBIDDEN {
            return self.stamp_insufficient_scope(res);
        }
        if res.status() != StatusCode::UNAUTHORIZED {
            return;
        }
        if res.extensions().get::<NoBearerChallenge>().is_some() {
            return;
        }
        // Never touch a non-`Bearer` scheme or a challenge already carrying the
        // pointer; otherwise merge the pointer in, keeping RFC 6750 §3.1's `error`.
        let merged = match res.headers().get(header::WWW_AUTHENTICATE) {
            // Nothing written yet (the guard-denial path): the full challenge,
            // with the RFC 6750 §3.1 code the guard recorded on the request.
            None => Cow::Borrowed(self.metadata.challenge()),
            Some(existing) => {
                let Ok(existing) = existing.to_str() else {
                    return;
                };
                let existing = existing.trim();
                let is_bearer = existing
                    .get(..BEARER.len())
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case(BEARER));
                if !is_bearer || existing.contains("resource_metadata") {
                    return;
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
            // Unreachable: every component was character-checked at boot. The
            // challenge stays out of the line: its pointer holds the resource's URI.
            Err(error) => tracing::error!(
                target: crate::TARGET,
                %error,
                "protected resource challenge is not a valid header value",
            ),
        }
    }

    /// The `403` half of RFC 6750 §3.1: a token that verified but is too narrow.
    /// Only a response carrying [`RequiredScopes`] qualifies; an ordinary `403`
    /// cannot be fixed by a wider token, so it gets no challenge.
    fn stamp_insufficient_scope(&self, res: &mut Response) {
        let Some(required) = res.extensions().get::<RequiredScopes>() else {
            return;
        };
        if required.is_empty() {
            return;
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

        // The scopes are the rule's literals, never a client's value; the edge
        // filed this line already where it stamped the step-up itself.
        if !required.iter().all(|scope| is_scope_token(scope)) && !carries_step_up(res) {
            tracing::warn!(
                target: crate::TARGET,
                scopes = ?required,
                "a required scope is not a scope-token; the challenge is sent without its scope \
                 attribute",
            );
        }
        let challenge = self.metadata.insufficient_scope_challenge(&required);
        match HeaderValue::from_str(&challenge) {
            Ok(value) => {
                res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
            }
            // Unreachable: the boot checked the pointer, and a scope outside the
            // grammar is left out. The challenge stays out of the line, as above.
            Err(error) => tracing::error!(
                target: crate::TARGET,
                %error,
                "insufficient-scope challenge is not a valid header value",
            ),
        }
    }
}

/// Whether `res` already carries RFC 6750 §3.1's `insufficient_scope` challenge.
fn carries_step_up(res: &Response) -> bool {
    let Some(Ok(existing)) = res
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .map(HeaderValue::to_str)
    else {
        return false;
    };
    existing
        .get(..BEARER.len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(BEARER))
        && existing.contains(&format!(r#"error="{INSUFFICIENT_SCOPE}""#))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge_for(res: Response) -> Option<String> {
        challenge_advertising(res, Vec::new())
    }

    fn challenge_advertising(mut res: Response, scopes_supported: Vec<String>) -> Option<String> {
        let metadata = ProtectedResourceMetadata::new(
            "https://api.example.com".into(),
            vec!["https://auth.example.com".into()],
            scopes_supported,
            vec!["header".into()],
            Default::default(),
        );
        ResourceChallenge {
            metadata: Arc::new(metadata),
        }
        .stamp(&mut res);
        res.headers()
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

    /// A scope outside the grammar is said once: here for a `403` built by
    /// hand, never again where the edge stamped the step-up and said it.
    #[test]
    fn a_scope_outside_the_grammar_is_said_where_no_edge_said_it() {
        const LEFT_OUT: &str = "a required scope is not a scope-token; the challenge is sent \
                                without its scope attribute";
        let outside = || {
            let mut res = status(StatusCode::FORBIDDEN);
            res.extensions_mut()
                .insert(RequiredScopes::new(["posts write"]));
            res
        };

        let logs = nest_rs_testing::LogCapture::install();
        let hand_built = challenge_for(outside()).expect("a step-up challenge");
        assert!(!hand_built.contains("scope=\""), "{hand_built}");

        let mut stamped = outside();
        stamped.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer error=\"insufficient_scope\""),
        );
        let challenge = challenge_for(stamped).expect("a step-up challenge");
        assert!(challenge.contains("resource_metadata="), "{challenge}");

        let event = logs.expect_one(crate::TARGET, LEFT_OUT);
        assert_eq!(event.level, "warn");
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
