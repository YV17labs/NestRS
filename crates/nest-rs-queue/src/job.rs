//! [`Job`] — the bound every queue payload satisfies.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// A queue payload: JSON-round-trippable and safe to move across tasks.
///
/// Implemented for every type meeting the bounds, so a payload writes no impl.
/// Backends carry jobs as JSON at the port — the open contract — which is why
/// every `Job` is `Serialize + DeserializeOwned`; a backend preferring a binary
/// codec internally still receives and returns this JSON shape.
pub trait Job: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}

impl<T> Job for T where T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}
