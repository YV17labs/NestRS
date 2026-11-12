//! MCP's reading of [`Scoped<T>`], the request-scoped provider an operation
//! `#[tools]` declares takes as a parameter.
//!
//! An operation receives no HTTP request: the endpoint carries the request's
//! scope onto rmcp's task, and `#[tools]` resolves each `Scoped<T>` from it
//! once the operation's guards and class gate admitted the caller.
//!
//! ```
//! # use nest_rs_core::{Scoped, injectable, module};
//! # use nest_rs_mcp::{DEFAULT_PATH, McpError, McpAccess, McpModule, mcp, tools};
//! # use nest_rs_testing::mcp::{call_tool, result};
//! #
//! # #[injectable(scope = request)]
//! # #[derive(Default)]
//! # struct RequestCache;
//! #
//! # impl RequestCache {
//! #     fn value(&self) -> &'static str {
//! #         "cached"
//! #     }
//! # }
//! #
//! # #[mcp]
//! # #[derive(Default)]
//! # struct CacheHost;
//! #
//! # #[tools]
//! # impl CacheHost {
//! #[tool(description = "Read this operation's cache.")]
//! #[public]
//! async fn do_it(&self, cache: Scoped<RequestCache>) -> Result<String, McpError> {
//!     Ok(cache.value().to_owned())
//! }
//! # }
//! #
//! # #[module(imports = [McpModule::for_root(McpAccess::Public)], providers = [RequestCache, CacheHost])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
//! # let body = call_tool(app.http(), DEFAULT_PATH, "do_it", None).await;
//!
//! assert_eq!(result(&body)["result"]["content"][0]["text"], "cached");
//! # Ok(())
//! # }
//! ```

use std::any::type_name;
use std::sync::Arc;

use nest_rs_core::{Failure, RequestScope, Scoped};

use crate::error::{OperationFailure, ScopeError};

/// The ambient per-operation scope, if one is installed.
pub(crate) fn current_scope() -> Option<Arc<RequestScope>> {
    nest_rs_core::current_request_scope()
}

/// The `Scoped<T>` an operation takes, from its request scope — emitted by
/// `#[tools]` in the operation's tail.
///
/// A miss is a failure the edge answers opaquely, its chain naming the
/// provider on the operator's line alone.
pub fn scoped_parameter<T: Send + Sync + 'static>() -> Result<Scoped<T>, OperationFailure> {
    let provider = type_name::<T>();
    let refused = |error: ScopeError| OperationFailure::unanswered(Failure::from(error));
    let scope = current_scope().ok_or_else(|| refused(ScopeError::NotInstalled { provider }))?;
    scope
        .get::<T>()
        .map(Scoped)
        .ok_or_else(|| refused(ScopeError::NoProvider { provider }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nest_rs_core::{Container, Correlation};

    use super::*;

    struct Probe(u64);

    fn scoped_container() -> Container {
        let counter = Arc::new(AtomicU64::new(0));
        Container::builder()
            .provide_scoped::<Probe, _>(move |_| Probe(counter.fetch_add(1, Ordering::SeqCst)))
            .build()
            .expect("the container builds")
    }

    fn resolved() -> Arc<Probe> {
        match scoped_parameter::<Probe>() {
            Ok(probe) => probe.into_inner(),
            Err(_) => panic!("the scope is installed and provides the probe"),
        }
    }

    #[tokio::test]
    async fn one_instance_serves_an_operation() {
        let scope = Arc::new(RequestScope::new(scoped_container()));
        nest_rs_core::with_request_scope(Some(scope), Correlation::minted(None), async {
            let (a, b) = (resolved(), resolved());
            assert!(Arc::ptr_eq(&a, &b));
        })
        .await;
    }

    #[tokio::test]
    async fn separate_operations_build_distinct_instances() {
        let container = scoped_container();
        let operation = |container: Container| {
            nest_rs_core::with_request_scope(
                Some(Arc::new(RequestScope::new(container))),
                Correlation::minted(None),
                async { resolved().0 },
            )
        };
        let first = operation(container.clone()).await;
        let second = operation(container).await;
        assert_ne!(first, second, "each operation builds its own instance");
    }

    #[test]
    fn a_miss_answers_no_type_name_and_files_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let Err(missed) = scoped_parameter::<Probe>() else {
            panic!("no scope is installed");
        };

        let answered = missed.into_answer();

        assert_eq!(answered.message, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        assert!(!format!("{answered:?}").contains("Probe"), "{answered:?}");
        let event = logs.expect_one(crate::TARGET, "mcp operation failed");
        assert!(
            event
                .field("error")
                .is_some_and(|chain| chain.contains("Probe")),
            "the operator reads the provider: {:?}",
            event.fields,
        );
    }
}
