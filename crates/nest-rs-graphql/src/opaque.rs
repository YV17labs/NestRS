//! What a failing operation tells the client, and what it tells the operator — the
//! GraphQL half of the seam MCP established.
//!
//! An operation's error frame is built from whatever the resolver's error type
//! prints, and `Display` is the wrong default for that: a `DbErr` carries SQL,
//! column names and sometimes row values. The framework's own `ServiceError::Db`
//! is `#[error("database error")]` so nothing leaks today, but a feature's own
//! error type has no such discipline imposed on it — and a GraphQL client is
//! exactly as untrusted as a language model.
//!
//! ```ignore
//! #[query]
//! #[authorize(Read, rooms::Entity)]
//! async fn rooms(&self) -> Result<Vec<Room>> {
//!     Ok(self.svc.list().await.opaque()?)
//! }
//! ```
//!
//! **A deliberate error is not this.** A validation rejection, a `Denial`, a
//! message a client can act on — those exist to be *read*. Return them directly;
//! `denial_to_graphql_error` is the shape a refusal already travels through.

use std::fmt::Display;

use async_graphql::Error as GraphqlError;
use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error is printable, so it covers a
/// `DbErr`, a storage error, an `anyhow::Error` and a feature's own type without
/// any of them having to know GraphQL exists.
///
/// The twin traits on MCP and WS are deliberately separate types rather than one
/// trait generic over the error: the output is what lets `.opaque()?` infer from
/// the enclosing resolver's return type. See `nest_rs_core::opaque`.
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the client an opaque one.
    fn opaque(self) -> Result<T, GraphqlError>;
}

impl<T, E: Display> Opaque<T> for Result<T, E> {
    fn opaque(self) -> Result<T, GraphqlError> {
        self.map_err(|err| {
            tracing::error!(
                target: "nest_rs::graphql",
                error = %err,
                "graphql operation failed",
            );
            // Extended with the same `INTERNAL` code an internal denial carries, so
            // a client cannot tell an unexpected failure from a refused one — which
            // is the point of both.
            use async_graphql::ErrorExtensions;
            GraphqlError::new(OPAQUE_CLIENT_MESSAGE).extend_with(|_, e| e.set("code", "INTERNAL"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error whose `Display` carries exactly what must not ship.
    struct Leaky;

    impl Display for Leaky {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("SELECT password_hash FROM \"user\" WHERE email = 'a@b.test'")
        }
    }

    #[test]
    fn the_frame_carries_a_constant_not_the_error() {
        let out: Result<(), GraphqlError> = Err(Leaky).opaque();
        let err = out.expect_err("the failure stays a failure");
        assert_eq!(err.message, OPAQUE_CLIENT_MESSAGE);
        assert!(
            !err.message.contains("password_hash"),
            "the whole point: a `Display` carrying SQL does not reach the wire",
        );
    }

    #[test]
    fn the_code_matches_what_an_internal_denial_carries() {
        let out: Result<(), GraphqlError> = Err(Leaky).opaque();
        let err = out.expect_err("a failure");
        let extensions = err.extensions.expect("the code rides as an extension");
        assert_eq!(
            extensions.get("code").and_then(|v| match v {
                async_graphql::Value::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("INTERNAL"),
            "one code for both, so a client cannot distinguish an unexpected \
             failure from a refusal it was not owed an explanation for",
        );
    }

    #[test]
    fn a_success_passes_through_untouched() {
        let out: Result<i32, GraphqlError> = Ok::<_, Leaky>(7).opaque();
        assert_eq!(out.ok(), Some(7));
    }
}
