//! The [`InterceptorExt`] extension trait — adds `.interceptor(_)` to any
//! poem endpoint.

use poem::{Endpoint, IntoResponse};

use crate::interceptor::{Interceptor, InterceptorEndpoint};

/// Extension methods that wrap a poem endpoint in an [`Interceptor`]. Bring
/// into scope to chain `.interceptor(_)`.
pub trait InterceptorExt: Endpoint + Sized + Send + Sync
where
    Self::Output: IntoResponse,
{
    fn interceptor<I: Interceptor>(self, interceptor: I) -> InterceptorEndpoint<Self, I> {
        InterceptorEndpoint::new(self, interceptor)
    }
}

impl<E> InterceptorExt for E
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
}
