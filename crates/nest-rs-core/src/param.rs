//! What provider a handler's parameter resolves, known by its type — never by
//! how the type is spelled.
//!
//! An extractor that resolves a provider per request declares it by
//! implementing [`ResolvesProvider`]. A macro reading tokens cannot tell such a
//! type from any other, so the impl half asks this probe per parameter, through
//! autoref: [`ParamProbeResolves`] is implemented for `&ParamProbe<T>` where
//! `T` resolves a provider, [`ParamProbeFallback`] for every `ParamProbe<T>`,
//! and a call on `&&ParamProbe<T>` takes the first whenever it applies. A
//! parameter typed with a generic parameter resolves to the fallback.

use std::any::TypeId;
use std::marker::PhantomData;

/// An extractor that resolves a provider from the request's scope — what the
/// access graph checks.
///
/// A handler parameter whose type implements it is checked at boot like an
/// `#[inject]` field — its provider must be offered by the host's module, a
/// module it imports, or global infrastructure — but never for scope: its
/// consumer is a request, so a request-scoped provider is the point.
///
/// ```
/// use std::any::TypeId;
/// use nest_rs_core::ResolvesProvider;
///
/// struct Tenant;
/// struct CurrentTenant;
///
/// impl ResolvesProvider for CurrentTenant {
///     fn provider() -> (TypeId, &'static str) {
///         (TypeId::of::<Tenant>(), "Tenant")
///     }
/// }
/// # assert_eq!(CurrentTenant::provider().0, TypeId::of::<Tenant>());
/// ```
pub trait ResolvesProvider {
    /// The container key of the provider this extractor resolves, and the label
    /// a boot error names it by.
    fn provider() -> (TypeId, &'static str);
}

/// A handler parameter's type, held so the compiler can be asked whether it
/// resolves a provider.
pub struct ParamProbe<T>(PhantomData<fn() -> T>);

impl<T> ParamProbe<T> {
    /// The probe for the type `T`.
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for ParamProbe<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`ParamProbe`] answers for a type that resolves a provider.
pub trait ParamProbeResolves {
    /// The provider the parameter resolves.
    fn resolved_provider(&self) -> Option<(TypeId, &'static str)>;
}

impl<T: ResolvesProvider> ParamProbeResolves for &ParamProbe<T> {
    fn resolved_provider(&self) -> Option<(TypeId, &'static str)> {
        Some(T::provider())
    }
}

/// What [`ParamProbe`] answers for every other type.
pub trait ParamProbeFallback {
    /// None: the parameter resolves no provider.
    fn resolved_provider(&self) -> Option<(TypeId, &'static str)>;
}

impl<T> ParamProbeFallback for ParamProbe<T> {
    fn resolved_provider(&self) -> Option<(TypeId, &'static str)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::{ParamProbe, ParamProbeFallback as _, ParamProbeResolves as _, ResolvesProvider};

    struct Tenant;

    struct CurrentTenant;

    impl ResolvesProvider for CurrentTenant {
        fn provider() -> (TypeId, &'static str) {
            (TypeId::of::<Tenant>(), "Tenant")
        }
    }

    /// An extractor generic over what it resolves, as an edge's `Scoped<T>` is.
    struct Scoped<T>(T);

    impl<T: 'static> ResolvesProvider for Scoped<T> {
        fn provider() -> (TypeId, &'static str) {
            (TypeId::of::<T>(), std::any::type_name::<T>())
        }
    }

    /// The expansion's call, written once so every test asks at the same
    /// reference depth.
    macro_rules! probe {
        ($ty:ty) => {
            (&&ParamProbe::<$ty>::new()).resolved_provider()
        };
    }

    #[test]
    fn a_type_that_resolves_a_provider_answers_it() {
        assert_eq!(
            probe!(CurrentTenant),
            Some((TypeId::of::<Tenant>(), "Tenant"))
        );
        assert_eq!(
            probe!(Scoped<Tenant>).map(|(id, _)| id),
            Some(TypeId::of::<Tenant>())
        );
    }

    #[test]
    fn a_type_that_resolves_none_answers_none() {
        assert_eq!(probe!(u32), None);
        assert_eq!(probe!(String), None);
        assert_eq!(probe!(&'static str), None);
        assert_eq!(probe!(Tenant), None);
    }

    /// Nothing proves the bound inside a generic function, whatever the type
    /// it is called with: the parameter resolves to the fallback.
    #[test]
    fn a_parameter_typed_with_a_generic_parameter_answers_none() {
        fn generic<T>() -> Option<(TypeId, &'static str)> {
            probe!(T)
        }
        assert_eq!(generic::<CurrentTenant>(), None);
        assert_eq!(generic::<u32>(), None);
    }

    /// The bound is read on the parameter's own type, so autoref never lends a
    /// reference the extractor's answer.
    #[test]
    fn a_reference_to_an_extractor_answers_none() {
        assert_eq!(probe!(&'static CurrentTenant), None);
        assert_eq!(probe!(&'static Scoped<Tenant>), None);
    }
}
