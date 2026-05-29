use std::sync::Arc;

use nestrs_middleware::Interceptor;

/// Discovery metadata attached by the `#[interceptor]` macro — the **global**
/// interceptor form. The [`crate::HttpTransport`] walks these at boot via
/// [`nestrs_core::DiscoveryService::meta`] and folds them around the
/// assembled route, *innermost* to *outermost* in registration order
/// (the last interceptor declared across the module tree wraps the rest). This
/// is for infrastructure that must wrap *everything* (a DB-transaction context,
/// tracing). To bind an interceptor to a single controller or handler instead,
/// write a plain `#[injectable] + impl Interceptor` and list it in
/// `#[use_interceptors(...)]` — it is then resolved from the container per route,
/// not auto-mounted globally.
pub struct HttpInterceptorMeta {
    interceptor: Arc<dyn Interceptor>,
}

impl HttpInterceptorMeta {
    pub fn new(interceptor: Arc<dyn Interceptor>) -> Self {
        Self { interceptor }
    }

    pub fn interceptor(&self) -> Arc<dyn Interceptor> {
        self.interceptor.clone()
    }
}
