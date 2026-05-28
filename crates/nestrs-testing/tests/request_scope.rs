//! Request-scoped providers (`#[injectable(scope = request)]`): a fresh instance
//! per request, cached for the life of that request, resolved in a handler via
//! the `Scoped<T>` extractor — driven end-to-end through the in-process harness.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use nestrs_core::{injectable, module};
use nestrs_http::{controller, routes, Scoped};
use nestrs_testing::TestApp;

/// A singleton: hands out a monotonically increasing number. Shared across all
/// requests, so it counts how many request-scoped `RequestId`s were *built*.
#[injectable]
#[derive(Default)]
struct Sequence {
    next: AtomicU64,
}

impl Sequence {
    fn take(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Request-scoped: injects the singleton `Sequence` and lazily claims one number
/// the first time `id()` is called. Because the scope caches the instance for the
/// request, every `Scoped<RequestId>` in one request shares this instance — so
/// the number is claimed exactly once per request.
#[injectable(scope = request)]
struct RequestId {
    #[inject]
    seq: Arc<Sequence>,
    id: OnceLock<u64>,
}

impl RequestId {
    fn id(&self) -> u64 {
        *self.id.get_or_init(|| self.seq.take())
    }
}

#[controller(path = "/")]
struct ScopeController;

#[routes]
impl ScopeController {
    // Two independent `Scoped<RequestId>` extractions in one request: if the scope
    // caches, both are the same instance (same id, pointer-equal).
    #[get("/id")]
    async fn id(&self, a: Scoped<RequestId>, b: Scoped<RequestId>) -> String {
        format!("{}-{}-{}", a.id(), b.id(), Arc::ptr_eq(&a.0, &b.0))
    }
}

#[module(providers = [Sequence, RequestId, ScopeController])]
struct ScopeModule;

#[tokio::test]
async fn instance_is_cached_within_a_request_and_fresh_across_requests() {
    let app = TestApp::for_module::<ScopeModule>().await.expect("boots");

    // First request: one instance built (id 1), shared by both extractions.
    let first = app.http().get("/id").send().await;
    first.assert_status_is_ok();
    first.assert_text("1-1-true").await;

    // Second request: a brand-new instance (id 2), again shared within the request.
    let second = app.http().get("/id").send().await;
    second.assert_status_is_ok();
    second.assert_text("2-2-true").await;
}
