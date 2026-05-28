//! Per-request resolution for request-scoped providers.
//!
//! The container is a flat singleton store; a `#[injectable(scope = request)]`
//! provider is the exception — it must be built fresh per request. A
//! [`RequestScope`] is that per-request layer: a transport creates one at the
//! start of each request (the HTTP transport inserts it into the poem request,
//! read back by the `Scoped<T>` extractor), and it lazily builds each
//! request-scoped provider **once**, caching the instance for the rest of the
//! request. A type that is *not* request-scoped falls through to the singleton
//! container, so a `RequestScope` resolves anything the container can plus the
//! request-scoped providers.
//!
//! The model is deliberately one level deep: a request-scoped provider resolves
//! its `#[inject]` dependencies from the singleton root, so it may depend on
//! singletons but not on other request-scoped providers (which would need the
//! scope threaded through construction). Singleton → request-scoped injection is
//! likewise unsupported: a singleton is built once, before any request exists.
//! Reach a request-scoped provider through the request boundary (`Scoped<T>`),
//! never a `#[inject]` field on a singleton.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::Container;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// A request-scoped resolution layer over the singleton [`Container`]. Cheap to
/// create (an empty cache); built once per request by the serving transport.
pub struct RequestScope {
    root: Container,
    cache: Mutex<HashMap<TypeId, AnyArc>>,
}

impl RequestScope {
    /// Open a fresh scope over the singleton container — one per request.
    pub fn new(root: Container) -> Self {
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The singleton container this scope layers over.
    pub fn root(&self) -> &Container {
        &self.root
    }

    /// Resolve `T`. A request-scoped provider is built once and cached for this
    /// scope; anything else falls through to the singleton container. Returns
    /// `None` when no provider (scoped or singleton) is registered for `T`.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        match self.root.scoped_factory(id) {
            Some(factory) => {
                let mut cache = self.cache.lock().expect("request scope cache poisoned");
                let any = cache
                    .entry(id)
                    .or_insert_with(|| factory(&self.root))
                    .clone();
                any.downcast::<T>().ok()
            }
            None => self.root.get::<T>(),
        }
    }
}
