//! A [`Denial`] as the edges with no status line answer it — GraphQL, a
//! WebSocket frame, a JSON-RPC error: one sentence, one class, and the evidence
//! a client acts on, worded once for every in-band edge; its class
//! ([`denial_code`]) is every edge's, HTTP's problem document included.

use nest_rs_core::Code;
use nest_rs_core::problem::code;

use crate::Denial;

/// The one sentence an in-band edge answers a denial with: a `4xx`'s own
/// reason, and a `5xx`'s condition — never what the refusal said of the server.
pub fn client_message(denial: &Denial) -> &str {
    match denial {
        Denial::Internal(_) => nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        Denial::Unavailable { .. } => nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
        _ => denial.message(),
    }
}

/// The [`Code`] a denial is known by on every edge: the `code` member of
/// HTTP's problem document, and on the edges with no status line the member a
/// `Problem`'s class rides under there too.
pub fn denial_code(denial: &Denial) -> Code {
    match denial {
        Denial::Unauthorized(_) | Denial::InvalidCredential { .. } => code::UNAUTHENTICATED,
        Denial::Forbidden(_) => code::FORBIDDEN,
        Denial::InsufficientScope { .. } => code::INSUFFICIENT_SCOPE,
        Denial::RateLimited { .. } => code::RATE_LIMITED,
        Denial::Unavailable { .. } => code::UNAVAILABLE,
        Denial::Internal(_) => code::INTERNAL,
    }
}

/// The machine-readable half of a refusal, for the edges with no status line
/// to carry it: the `code` a client branches on, the member a `Problem`
/// answers under, and the same class as `reason`, the member these edges
/// carried first; plus `requiredScopes` when the denial names them and
/// `retryAfterSeconds` when it names a wait.
pub fn structured_reason(denial: &Denial) -> serde_json::Map<String, serde_json::Value> {
    let reason = match denial {
        Denial::Unauthorized(_) | Denial::InvalidCredential { .. } => "unauthenticated",
        Denial::Forbidden(_) => "forbidden",
        Denial::InsufficientScope { .. } => "insufficient_scope",
        Denial::RateLimited { .. } => "rate_limited",
        Denial::Unavailable { .. } => "unavailable",
        Denial::Internal(_) => "internal",
    };
    let mut data = serde_json::Map::new();
    data.insert(
        "code".to_owned(),
        serde_json::Value::from(denial_code(denial).as_str()),
    );
    data.insert("reason".to_owned(), serde_json::Value::from(reason));
    let scopes = denial.required_scopes();
    if !scopes.is_empty() {
        data.insert("requiredScopes".to_owned(), serde_json::json!(scopes));
    }
    // The wait, in delay-seconds like `Retry-After`, for edges with no status line.
    if let Some(retry_after_secs) = denial.retry_after_secs() {
        data.insert(
            "retryAfterSeconds".to_owned(),
            serde_json::Value::from(retry_after_secs),
        );
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All four edges report the wait, in one unit.
    #[test]
    fn every_edge_reports_the_wait_a_rate_limit_denial_carries() {
        let data = structured_reason(&Denial::rate_limited(42, "too many requests"));
        assert_eq!(data["reason"], "rate_limited");
        assert_eq!(
            data["retryAfterSeconds"], 42,
            "the in-band edges carry the wait: {data:?}",
        );

        // Only a rate limit and an unavailable dependency have one — a 403
        // carrying `retryAfterSeconds` would tell a client to retry something
        // that will never succeed.
        let forbidden = structured_reason(&Denial::forbidden("nope"));
        assert!(
            forbidden.get("retryAfterSeconds").is_none(),
            "only a denial that may clear names a wait: {forbidden:?}",
        );
    }

    /// A dependency that did not answer: reason `unavailable`, its wait
    /// reported when given and never invented.
    #[test]
    fn every_edge_reports_an_unavailable_dependency_and_its_known_wait() {
        let data = structured_reason(&Denial::unavailable(Some(7), "authentication unavailable"));
        assert_eq!(data["reason"], "unavailable");
        assert_eq!(data["retryAfterSeconds"], 7, "{data:?}");
        let unknown = structured_reason(&Denial::unavailable(None, "authentication unavailable"));
        assert_eq!(unknown["reason"], "unavailable");
        assert!(unknown.get("retryAfterSeconds").is_none(), "{unknown:?}");
    }

    #[test]
    fn a_server_side_denial_tells_the_client_its_condition_only() {
        assert_eq!(
            client_message(&Denial::internal("redis at 10.0.3.7 refused")),
            nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        );
        assert_eq!(
            client_message(&Denial::unavailable(Some(7), "store at 10.0.0.1 down")),
            nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
        );
        assert_eq!(client_message(&Denial::forbidden("not yours")), "not yours");
    }
}
