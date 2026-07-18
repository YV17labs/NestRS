//! The [`FilterExt`] extension trait — adds `.filter(_)` to any poem endpoint.

use poem::{Endpoint, IntoResponse};

use crate::filter::{Filter, FilterEndpoint};

/// Extension methods that wrap a poem endpoint in a [`Filter`]. Bring into
/// scope to chain `.filter(_)`.
pub trait FilterExt: Endpoint + Sized + Send + Sync
where
    Self::Output: IntoResponse,
{
    /// Wrap this endpoint in `filter`, returning the wrapped endpoint.
    fn filter<F: Filter>(self, filter: F) -> FilterEndpoint<Self, F> {
        FilterEndpoint::new(self, filter)
    }
}

impl<E> FilterExt for E
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
}
