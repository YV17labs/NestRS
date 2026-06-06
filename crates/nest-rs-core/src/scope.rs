//! Per-request resolution for request-scoped providers.
//!
//! The container is a flat singleton store; a `#[injectable(scope = request)]`
//! provider is the exception — built fresh per request and cached for that
//! request by a [`RequestScope`]. Non-scoped types fall through to the
//! singleton container.
//!
//! The model is one level deep: a request-scoped provider depends on
//! singletons, never on other request-scoped providers; singletons cannot
//! depend on a request-scoped provider (they're built before any request
//! exists). Reach a request-scoped provider through the request boundary
//! (`Scoped<T>`), never a `#[inject]` field on a singleton.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::Container;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// Request-scoped resolution layer over the singleton [`Container`]. Built
/// once per request by the serving transport.
pub struct RequestScope {
    root: Container,
    cache: Mutex<HashMap<TypeId, AnyArc>>,
}

impl RequestScope {
    pub fn new(root: Container) -> Self {
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Container {
        &self.root
    }

    /// Resolve `T`. Request-scoped providers are built once and cached for
    /// this scope; transient providers are rebuilt on every call; non-scoped
    /// types fall through to the singleton container.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        if let Some(factory) = self.root.scoped_factory(id) {
            let mut cache = self.cache.lock();
            let any = cache
                .entry(id)
                .or_insert_with(|| factory(&self.root))
                .clone();
            return any.downcast::<T>().ok();
        }
        // Transients route through `Container::get` so the re-entrancy guard
        // catches a self-cycle, regardless of which surface initiates the call.
        self.root.get::<T>()
    }
}
