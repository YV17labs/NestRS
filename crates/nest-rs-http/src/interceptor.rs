use std::sync::Arc;

use nest_rs_middleware::Interceptor;

/// Discovery metadata attached by the `#[interceptor]` macro — the **global**
/// interceptor form, folded around the assembled route innermost-to-outermost
/// in registration order. For infrastructure that must wrap everything (DB
/// transaction context, tracing). To bind per-controller/handler, write a
/// plain `#[injectable] + impl Interceptor` and list it in `#[use_interceptors]`.
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
