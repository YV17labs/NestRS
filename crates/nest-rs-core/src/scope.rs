//! [`Scoped<T>`] — a handler's parameter resolving a request-scoped provider,
//! the one type every edge reads.

use std::any::{TypeId, type_name};
use std::borrow::Cow;
use std::ops::Deref;
use std::sync::Arc;

use crate::type_name::short_static;
use crate::{CarriesRemedy, ResolvesProvider};

/// A handler's parameter resolving a provider of type `T` from the running
/// unit's `RequestScope` — above all an
/// `#[injectable(scope = request)]` one, a singleton falling through to the
/// container.
///
/// One parameter on every edge: an HTTP route, a GraphQL operation or field
/// resolver, a WebSocket message and an MCP operation each resolve it, and the
/// edges carrying operations leave it out of the arguments their protocol
/// describes. The provider is part of the boot's access contract
/// ([`ResolvesProvider`]). Should it still be missing at a unit — no scope
/// installed, or `T` offered by no module — the unit answers opaquely and the
/// type is filed at `error`, never answered.
///
/// ```
/// use std::any::TypeId;
///
/// use nest_rs_core::{ResolvesProvider, Scoped};
///
/// struct Cart;
///
/// assert_eq!(Scoped::<Cart>::provider().0, TypeId::of::<Cart>());
/// ```
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    /// Take the resolved provider handle out of the parameter.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: 'static> ResolvesProvider for Scoped<T> {
    fn provider() -> (TypeId, &'static str) {
        (TypeId::of::<T>(), short_static(type_name::<T>()))
    }
}

impl<T: CarriesRemedy> CarriesRemedy for Scoped<T> {
    fn remedy() -> Cow<'static, str> {
        T::remedy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__private::{ParamProbe, ParamProbeFallback as _, ParamProbeResolves as _};

    struct Marker(&'static str);

    #[test]
    fn into_inner_yields_the_arc() {
        let value = Arc::new(Marker("hi"));
        let scoped = Scoped(value.clone());
        assert!(Arc::ptr_eq(&scoped.into_inner(), &value));
    }

    #[test]
    fn it_derefs_to_the_provider() {
        let scoped = Scoped(Arc::new(Marker("bye")));
        assert_eq!(scoped.0.as_ref().0, "bye");
        assert_eq!((*scoped).0, "bye");
    }

    /// The probe every impl half asks per parameter, at the reference depth its
    /// expansion calls it.
    macro_rules! probe {
        ($ty:ty) => {
            (&&ParamProbe::<$ty>::new()).resolved_provider()
        };
    }

    #[test]
    fn the_impl_half_probe_reads_the_provider_it_resolves() {
        let resolved = probe!(Scoped<Marker>);
        assert_eq!(
            resolved,
            Some((TypeId::of::<Marker>(), "Marker")),
            "named as a refusal names a type, by its short name"
        );
        assert_eq!(probe!(Arc<Marker>), None);
        assert_eq!(probe!(&Scoped<Marker>), None, "a borrow resolves nothing");
    }

    struct Opened;

    impl CarriesRemedy for Opened {
        fn remedy() -> Cow<'static, str> {
            "Import the module opening it.".into()
        }
    }

    /// The remedy probe every impl half asks per parameter, at the reference
    /// depth its expansion calls it.
    macro_rules! remedy {
        ($ty:ty) => {
            (&&crate::__private::RemedyProbe::<$ty>::new())
                .carried_remedy()
                .map(|remedy| remedy())
        };
    }

    #[test]
    fn a_parameter_carries_the_remedy_of_the_provider_it_resolves() {
        use crate::__private::{RemedyProbeCarries as _, RemedyProbeFallback as _};

        assert_eq!(
            remedy!(Scoped<Opened>),
            Some(Cow::Borrowed("Import the module opening it."))
        );
        assert_eq!(remedy!(Scoped<Marker>), None);
    }
}
