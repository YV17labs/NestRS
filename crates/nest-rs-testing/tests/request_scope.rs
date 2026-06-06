//! `#[injectable(scope = request)]`: fresh instance per request, cached for
//! the life of that request, resolved with `Scoped<T>`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use nest_rs_core::{injectable, module};
use nest_rs_http::{Scoped, controller, routes};
use nest_rs_testing::TestApp;

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
    /// Two extractions in one request: if scope caches, both Arcs point to the
    /// same instance (same id, ptr_eq true).
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

    let first = app.http().get("/id").send().await;
    first.assert_status_is_ok();
    first.assert_text("1-1-true").await;

    let second = app.http().get("/id").send().await;
    second.assert_status_is_ok();
    second.assert_text("2-2-true").await;
}
