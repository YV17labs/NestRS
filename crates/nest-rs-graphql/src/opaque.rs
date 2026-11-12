//! A failure the client is owed no word of, said where it is met: `.opaque()`
//! turns any error into the `async_graphql::Error` a resolver body returns,
//! answering [`OPAQUE_CLIENT_MESSAGE`](nest_rs_core::OPAQUE_CLIENT_MESSAGE)
//! under the `INTERNAL` code, whatever the error's type would otherwise have
//! answered, and keeping the whole chain as its `source`, which the edge files
//! at `error` naming the root field once that error is the answer.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_graphql::async_graphql::{Result, SimpleObject};
//! # use nest_rs_graphql::{GraphqlModule, Opaque, operations, resolver};
//! #
//! # #[derive(SimpleObject)]
//! # struct Room {
//! #     name: String,
//! # }
//! #
//! # #[injectable]
//! # #[derive(Default)]
//! # struct RoomsService;
//! #
//! # impl RoomsService {
//! #     async fn list(&self) -> Result<Vec<Room>, std::io::Error> {
//! #         Err(std::io::Error::other("relation \"rooms\" does not exist"))
//! #     }
//! # }
//! #
//! # #[resolver]
//! # struct RoomsResolver {
//! #     #[inject]
//! #     svc: Arc<RoomsService>,
//! # }
//! #
//! # #[operations]
//! # impl RoomsResolver {
//! #[query]
//! #[public]
//! async fn rooms(&self) -> Result<Vec<Room>> {
//!     Ok(self.svc.list().await.opaque()?)
//! }
//! # }
//! #
//! # #[module(imports = [GraphqlModule], providers = [RoomsService, RoomsResolver])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
//! # let resp = app.http().post("/graphql")
//! #     .body_json(&serde_json::json!({ "query": "{ rooms { name } }" })).send().await;
//! # let body: serde_json::Value = resp.json().await.value().deserialize();
//!
//! assert_eq!(body["errors"][0]["message"], nest_rs_core::OPAQUE_CLIENT_MESSAGE);
//! # Ok(())
//! # }
//! ```
//!
//! **A deliberate error is not this.** A validation rejection, a `Denial`, a
//! message a client can act on — those exist to be *read*. Return them directly;
//! `denial_to_graphql_error` is the shape a refusal already travels through.

use async_graphql::Error as GraphqlError;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error converts into a boxed error, so
/// the whole cause chain reaches the operator's line.
///
/// Separate from the MCP and WS twins: the output type is what lets `.opaque()?`
/// infer from the resolver's return type (see `nest_rs_core::opaque`).
pub trait Opaque<T> {
    /// Hand the client an opaque error, keeping the real one for the
    /// operator's line, which the edge files once it is the answer.
    fn opaque(self) -> Result<T, GraphqlError>;
}

impl<T, E> Opaque<T> for Result<T, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn opaque(self) -> Result<T, GraphqlError> {
        self.map_err(|err| crate::error::opaque_error(&*nest_rs_core::boxed_error(err)))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Display;

    use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

    use super::*;

    #[derive(Debug)]
    struct Leaky;

    impl std::error::Error for Leaky {}

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

    /// The chain rides the error as its source, for the edge to file once the
    /// answer is final: nothing at the call, and nothing of it on the wire.
    #[test]
    fn and_the_operator_gets_the_error_the_client_does_not() {
        let logs = nest_rs_testing::LogCapture::install();
        let err = Err::<(), _>(Leaky).opaque().expect_err("a failure");

        logs.expect_none("nest_rs::graphql", "graphql operation failed");
        let kept = crate::error::take_withheld(&err);
        assert!(
            matches!(&kept, Some(nest_rs_core::__private::Withheld::Failed(said)) if said.contains("password_hash")),
            "the cause the answer withholds is exactly what the line has to carry, got {kept:?}",
        );
        let wire = serde_json::to_string(&err.into_server_error(async_graphql::Pos::default()))
            .expect("serializes");
        assert!(!wire.contains("password_hash"), "{wire}");
    }
}
