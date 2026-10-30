//! [`Ctx<T>`]: what a guard attached, read as a handler's parameter on every
//! edge.

use std::ops::Deref;

use crate::ambient::{AmbientValue, ambient};
use crate::error::MissingAmbientError;

/// A handler's parameter reading the `T` a guard attached to its unit with
/// [`UnitContext::attach`](crate::UnitContext::attach) — the principal an
/// authentication guard verified, or what a guard after it derived from it.
///
/// One parameter on every edge: an HTTP route, a GraphQL operation or field
/// resolver, a WebSocket message and an MCP operation each read it from the
/// unit's ambient context once its guards passed, and the edges carrying
/// operations leave it out of the arguments their protocol describes. Absent,
/// the unit fails: the guard that attaches `T` did not run there, a wiring
/// defect the edge answers opaquely and files at `error`, naming the type —
/// but on an HTTP route where an authentication guard refused a presented
/// credential, which answers the `401` it deferred.
///
/// `T` is cloned out of the ambient context, so attach a value that is cheap to
/// clone. A guard reads what an earlier one attached with
/// [`UnitContext::require`](crate::UnitContext::require), and code a handler
/// calls with [`ambient`].
pub struct Ctx<T>(pub T);

impl<T> Ctx<T> {
    /// Take ownership of the attached value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Ctx<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// The `Ctx<T>` the running unit carries, or the error naming the type it
/// lacks: what every edge's parameter reads before answering a miss its own
/// way.
pub fn attached<T: AmbientValue + Clone>() -> Result<Ctx<T>, MissingAmbientError> {
    ambient::<T>()
        .map(|value| Ctx(T::clone(&value)))
        .ok_or_else(MissingAmbientError::of::<T>)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::with_ambient;

    #[derive(Clone)]
    struct Principal(&'static str);

    impl AmbientValue for Principal {}

    #[tokio::test]
    async fn an_attached_value_is_read() {
        let read = with_ambient(Arc::new(Principal("ada")), async {
            attached::<Principal>().map(Ctx::into_inner)
        })
        .await;
        assert_eq!(read.ok().map(|principal| principal.0), Some("ada"));
    }

    #[test]
    fn a_missing_value_names_its_type() {
        let Err(missing) = attached::<Principal>() else {
            panic!("nothing attached a principal");
        };
        assert!(missing.value.ends_with("::Principal"), "{}", missing.value);
    }
}
