//! A [`Denial`] as the edges with no status line answer it — GraphQL, a
//! WebSocket frame, a JSON-RPC error: one sentence, one class, and its class
//! in words, worded once for every in-band edge; its class ([`denial_code`])
//! is every edge's, HTTP's problem document included.

use nest_rs_core::Code;
use nest_rs_core::problem::code;

use crate::Denial;
use crate::denial::Class;

/// The one sentence an in-band edge answers a denial with: a `4xx`'s own
/// reason, and a `5xx`'s condition — never what the refusal said of the server.
pub fn client_message(denial: &Denial) -> &str {
    match denial.class() {
        Class::Internal => nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        Class::Unavailable { .. } => nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
        _ => denial.message(),
    }
}

/// The [`Code`] a denial is known by on every edge: the `code` member of
/// HTTP's problem document, and on the edges with no status line the member a
/// `Problem`'s class rides under there too.
pub fn denial_code(denial: &Denial) -> Code {
    match denial.class() {
        Class::Unauthorized | Class::InvalidCredential { .. } => code::UNAUTHENTICATED,
        Class::Forbidden => code::FORBIDDEN,
        Class::InsufficientScope { .. } => code::INSUFFICIENT_SCOPE,
        Class::RateLimited { .. } => code::RATE_LIMITED,
        Class::Unavailable { .. } => code::UNAVAILABLE,
        Class::Internal => code::INTERNAL,
    }
}

/// A denial's class in lowercase words: the `reason` WebSocket's frame and
/// MCP's error `data` carried first, and still carry beside its
/// [`denial_code`].
pub fn denial_reason(denial: &Denial) -> &'static str {
    match denial.class() {
        Class::Unauthorized | Class::InvalidCredential { .. } => "unauthenticated",
        Class::Forbidden => "forbidden",
        Class::InsufficientScope { .. } => "insufficient_scope",
        Class::RateLimited { .. } => "rate_limited",
        Class::Unavailable { .. } => "unavailable",
        Class::Internal => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each class has its word, and its code reads the same class.
    #[test]
    fn every_denial_names_its_class_in_words_and_by_its_code() {
        for (denial, reason, class) in [
            (
                Denial::unauthorized("no credential"),
                "unauthenticated",
                code::UNAUTHENTICATED,
            ),
            (
                Denial::invalid_credential("the token expired", "invalid_token"),
                "unauthenticated",
                code::UNAUTHENTICATED,
            ),
            (Denial::forbidden("not yours"), "forbidden", code::FORBIDDEN),
            (
                Denial::insufficient_scope(["posts:write"], "narrow"),
                "insufficient_scope",
                code::INSUFFICIENT_SCOPE,
            ),
            (
                Denial::rate_limited(42, "too many requests"),
                "rate_limited",
                code::RATE_LIMITED,
            ),
            (
                Denial::unavailable(Some(7), "authentication unavailable"),
                "unavailable",
                code::UNAVAILABLE,
            ),
            (Denial::internal("store down"), "internal", code::INTERNAL),
        ] {
            assert_eq!(denial_reason(&denial), reason, "{denial:?}");
            assert_eq!(denial_code(&denial), class, "{denial:?}");
        }
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
