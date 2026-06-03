use poem::{Endpoint, IntoResponse};

use crate::{
    filter::{Filter, FilterEndpoint},
    guard::{Guard, GuardEndpoint},
    interceptor::{Interceptor, InterceptorEndpoint},
};

/// Extension methods that wrap a poem endpoint in a named middleware
/// category. Bring into scope to chain `.interceptor(_)`, `.guard(_)`,
/// `.filter(_)`.
pub trait EndpointExt: Endpoint + Sized + Send + Sync
where
    Self::Output: IntoResponse,
{
    fn interceptor<I: Interceptor>(self, interceptor: I) -> InterceptorEndpoint<Self, I> {
        InterceptorEndpoint::new(self, interceptor)
    }

    fn guard<G: Guard>(self, guard: G) -> GuardEndpoint<Self, G> {
        GuardEndpoint::new(self, guard)
    }

    fn filter<F: Filter>(self, filter: F) -> FilterEndpoint<Self, F> {
        FilterEndpoint::new(self, filter)
    }
}

impl<E> EndpointExt for E
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
}
