//! A resolver's request-scoped providers — the GraphQL mirror of
//! [`nest_rs_http::Scoped<T>`]. A [`Scoped<T>`] parameter resolves an
//! `#[injectable(scope = request)]` provider from the request the operation
//! arrived on; the boot checks its provider like an `#[inject]`ed one.
//!
//! ```
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_graphql::{GraphqlModule, Scoped, operations, resolver};
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
//! # #[module(imports = [GraphqlModule::for_root(None)], providers = [PerRequestCache, WhoResolver])]
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
//! **Caveat.** A graphql-ws socket outlives the request that upgraded it and
//! carries no request scope, and a `#[dataloader]` batch runs off-task, shared
//! by every field of the request: a request-scoped provider is reachable in
//! neither. A batch reads the ambient values its request carries into it.

use std::any::{TypeId, type_name};
use std::ops::Deref;
use std::sync::Arc;

use nest_rs_core::{Failure, ResolvesProvider};

use crate::error::{FieldFailure, Unprovided};

/// Resolves a provider of type `T` from the request the operation arrived on,
/// as a resolver method's parameter.
///
/// The provider is part of the boot's access contract
/// ([`ResolvesProvider`]). Should it still be missing at an operation — the
/// operation came over a graphql-ws socket, or `T` is offered by no module —
/// the field answers the opaque internal error and the type is filed at
/// `error`, never answered.
///
/// ```
/// use std::any::TypeId;
///
/// use nest_rs_core::ResolvesProvider;
/// use nest_rs_graphql::Scoped;
///
/// struct Cart;
///
/// assert_eq!(Scoped::<Cart>::provider().0, TypeId::of::<Cart>());
/// ```
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    /// Take the resolved provider handle out of the wrapper.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: 'static> ResolvesProvider for Scoped<T> {
    fn provider() -> (TypeId, &'static str) {
        (TypeId::of::<T>(), type_name::<T>())
    }
}

/// A request-scoped parameter `#[operations]` resolves before the call.
pub trait FromScope: Sized {
    /// The provider from the running request's scope, or the field's failure.
    fn from_scope() -> Result<Self, FieldFailure>;
}

impl<T: Send + Sync + 'static> FromScope for Scoped<T> {
    fn from_scope() -> Result<Self, FieldFailure> {
        let provider = nest_rs_core::current_request_scope().and_then(|scope| scope.get::<T>());
        provider.map(Scoped).ok_or_else(unprovided::<T>)
    }
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
    use nest_rs_core::__private::{ParamProbe, ParamProbeFallback as _, ParamProbeResolves as _};
    use nest_rs_testing::LogCapture;

    use super::*;

    mod red {
        pub(super) struct Hidden;
    }

    /// The probe `#[operations]` asks per parameter, written once at the
    /// reference depth its expansion calls it.
    macro_rules! probe {
        ($ty:ty) => {
            (&&ParamProbe::<$ty>::new()).resolved_provider()
        };
    }

    #[test]
    fn a_scoped_parameter_declares_the_provider_it_resolves() {
        assert_eq!(
            probe!(Scoped<red::Hidden>).map(|(id, _)| id),
            Some(TypeId::of::<red::Hidden>()),
        );
        assert_eq!(probe!(i32), None);
    }

    #[test]
    fn a_miss_answers_no_type_name_and_files_it() {
        let logs = LogCapture::install();

        let Err(missed) = Scoped::<red::Hidden>::from_scope() else {
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
