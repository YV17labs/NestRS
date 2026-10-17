//! Which route the router matched, carried out of it to the edge: the span is
//! named for it once the request answered, and a request dropped before it
//! answered files its line under it.

use std::sync::{Arc, Mutex, PoisonError};

/// The template the router matched one request on, in nestrs's spelling, once
/// the table's entry has noted it.
#[derive(Clone, Default)]
pub(crate) struct MatchedRoute(Arc<Mutex<Option<Arc<str>>>>);

impl MatchedRoute {
    /// The template noted for this request, if the router matched one.
    pub(crate) fn route(&self) -> Option<Arc<str>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Note `template` as the one the router matched.
    pub(crate) fn note(&self, template: &Arc<str>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(template));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_shares_what_one_clone_noted() {
        let slot = MatchedRoute::default();
        assert_eq!(slot.route(), None);
        slot.clone().note(&Arc::from("/users/{id}"));
        assert_eq!(slot.route().as_deref(), Some("/users/{id}"));
    }
}
