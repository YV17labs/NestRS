//! GraphQL's reading of [`Scoped<T>`]: a resolver's parameter resolving an
//! `#[injectable(scope = request)]` provider from the request the operation
//! arrived on; the boot checks its provider like an `#[inject]`ed one.
//!
//! ```
//! # use nest_rs_core::{Scoped, injectable, module};
//! # use nest_rs_graphql::{GraphqlModule, operations, resolver};
//! #
//! # #[injectable(scope = request)]
//! # #[derive(Default)]
//! # struct PerRequestCache;
//! #
//! # impl PerRequestCache {
//! #     fn value(&self) -> &'static str {
//! #         "cached"
//! #     }
//! # }
//! #
//! # #[resolver]
//! # struct WhoResolver;
//! #
//! # #[operations]
//! # impl WhoResolver {
//! #[query]
//! #[public]
//! async fn who(&self, cache: Scoped<PerRequestCache>) -> String {
//!     cache.value().to_string()
//! }
//! # }
//! #
//! # #[module(imports = [GraphqlModule], providers = [PerRequestCache, WhoResolver])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
//! # let resp = app.http().post("/graphql")
//! #     .body_json(&serde_json::json!({ "query": "{ who }" })).send().await;
//!
//! resp.assert_json(serde_json::json!({ "data": { "who": "cached" } })).await;
//! # Ok(())
//! # }
//! ```
//!
//! Each operation a graphql-ws socket carries runs in a request scope of its
//! own, never its upgrade's: a query's or a mutation's for its unit, a
//! subscription's from its opening through every item. **Caveat.** A
//! `#[dataloader]` batch runs off-task, shared by every field of the request,
//! so a request-scoped provider is not reachable there; a batch reads the
//! ambient values its request carries into it.

use std::any::type_name;

use nest_rs_core::{Failure, Scoped};

use crate::error::{FieldFailure, Unprovided};

/// The `Scoped<T>` an operation takes, read in its tail once its guards
/// passed; a miss — one run outside any request, as a schema driven bare is —
/// answers the opaque internal error, the type filed at `error`, never
/// answered.
pub fn scoped_parameter<T: Send + Sync + 'static>() -> Result<Scoped<T>, FieldFailure> {
    let provider = nest_rs_core::current_request_scope().and_then(|scope| scope.get::<T>());
    provider.map(Scoped).ok_or_else(unprovided::<T>)
}

/// What a field reaching a provider that is not there fails with: the opaque
/// answer, the type on the operator's line.
pub fn unprovided<T: 'static>() -> FieldFailure {
    FieldFailure::unanswered(Failure::from(Unprovided {
        provider: type_name::<T>(),
    }))
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    mod red {
        pub(super) struct Hidden;
    }

    #[test]
    fn a_miss_answers_no_type_name_and_files_it() {
        let logs = LogCapture::install();

        let Err(missed) = scoped_parameter::<red::Hidden>() else {
            panic!("no request scope is installed here");
        };
        let answered = missed.into_answer();

        assert_eq!(answered.message, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        let said = serde_json::to_string(&answered.extensions).expect("extensions serialize");
        assert!(!said.contains("Hidden"), "{said}");
        let line = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert!(
            line.field("error")
                .is_some_and(|error| error.contains("red::Hidden")),
            "{:?}",
            line.fields,
        );
    }
}
