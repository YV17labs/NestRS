//! What provider a handler's parameter resolves, and what it reads off the
//! unit's ambient context, known by its type — never by how the type is spelled.
//!
//! An extractor that resolves a provider per request declares it by
//! implementing [`ResolvesProvider`]. A macro reading tokens cannot tell such a
//! type from any other, nor an [`Ambient`] behind an alias from another
//! extractor, so the impl half asks this probe per parameter, through autoref:
//! [`ParamProbeResolves`] is implemented for `&ParamProbe<T>` where `T`
//! resolves a provider, [`ParamProbeAmbient`] for `&ParamProbe<Ambient<T>>`,
//! [`ParamProbeFallback`] for every `ParamProbe<T>`, and a call on
//! `&&ParamProbe<T>` takes the first whenever it applies. A parameter typed
//! with a generic parameter resolves to the fallback.
//!
//! An edge carrying operations reads `Ambient<T>` and `Scoped<T>` by their
//! names, since its protocol's arguments are written before any type is known,
//! so it asks [`ParamProbe::decoded`] of every other parameter: an `Ambient` or
//! a `Scoped` met there is one an alias hid, refused with the fix.

use std::any::TypeId;
use std::marker::PhantomData;

use crate::ambient::Ambient;
use crate::scope::Scoped;

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

/// What [`ParamProbe`] answers for an [`Ambient`], under whatever name the
/// handler gives it.
pub trait ParamProbeAmbient {
    /// The type the parameter reads off the unit's ambient context.
    fn ambient_type(&self) -> Option<TypeId>;
}

impl<T: 'static> ParamProbeAmbient for &ParamProbe<Ambient<T>> {
    fn ambient_type(&self) -> Option<TypeId> {
        Some(TypeId::of::<T>())
    }
}

impl<T: TailParam> ParamProbe<T> {
    /// An `Ambient` or a `Scoped` the protocol would decode: refused, by a bound
    /// no type meets.
    pub fn decoded(&self)
    where
        T: ProtocolArgument,
    {
    }
}

/// A parameter the unit resolves rather than its protocol decoding it.
pub trait TailParam {}

impl<T> TailParam for Ambient<T> {}

impl<T> TailParam for Scoped<T> {}

/// What a parameter an edge carrying operations hands its protocol must be, and
/// an `Ambient` or a `Scoped` is not: the unit resolves those two.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is read by its name on this edge, and this parameter is named otherwise",
    label = "read as an argument the protocol decodes",
    note = "write `Ambient<T>` or `Scoped<T>` in the signature, not an alias of it: a GraphQL operation, a WebSocket message and an MCP operation leave a parameter so named out of their protocol's arguments and resolve it from the unit"
)]
pub trait ProtocolArgument {}

/// What [`ParamProbe`] answers for every other type.
pub trait ParamProbeFallback {
    /// None: the parameter resolves no provider.
    fn resolved_provider(&self) -> Option<(TypeId, &'static str)>;

    /// None: the parameter reads nothing a guard attached.
    fn ambient_type(&self) -> Option<TypeId>;

    /// Neither an `Ambient` nor a `Scoped`: nothing to refuse.
    fn decoded(&self) {}
}

impl<T> ParamProbeFallback for ParamProbe<T> {
    fn resolved_provider(&self) -> Option<(TypeId, &'static str)> {
        None
    }

    fn ambient_type(&self) -> Option<TypeId> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::{
        ParamProbe, ParamProbeAmbient as _, ParamProbeFallback as _, ParamProbeResolves as _,
        ResolvesProvider,
    };
    use crate::ambient::Ambient;

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

    /// The expansion's call for what a parameter reads off the ambient context.
    macro_rules! ambient {
        ($ty:ty) => {
            (&&ParamProbe::<$ty>::new()).ambient_type()
        };
    }

    /// What `Me` names is read, as `Ambient<Tenant>` is: no spelling decides.
    #[test]
    fn an_ambient_parameter_reads_its_type_under_whatever_name_it_has() {
        type Me = Ambient<Tenant>;
        assert_eq!(ambient!(Ambient<Tenant>), Some(TypeId::of::<Tenant>()));
        assert_eq!(ambient!(Me), Some(TypeId::of::<Tenant>()));
        assert_eq!(ambient!(crate::Ambient<u32>), Some(TypeId::of::<u32>()));
    }

    /// What may hold no value, or the value's place in another type, reads
    /// nothing a unit must carry.
    #[test]
    fn a_parameter_that_is_no_ambient_reads_nothing() {
        assert_eq!(ambient!(Option<Ambient<Tenant>>), None);
        assert_eq!(ambient!(Result<Ambient<Tenant>, String>), None);
        assert_eq!(ambient!(&'static Ambient<Tenant>), None);
        assert_eq!(ambient!(Scoped<Tenant>), None);
        assert_eq!(ambient!(Tenant), None);
    }

    /// Every type but an `Ambient` and a `Scoped` passes the in-band check; those
    /// two are refused at compile time, which the hygiene suite pins.
    #[test]
    fn a_type_the_unit_does_not_resolve_is_an_argument() {
        ParamProbe::<String>::new().decoded();
        ParamProbe::<Option<Ambient<Tenant>>>::new().decoded();
        ParamProbe::<&'static crate::Scoped<Tenant>>::new().decoded();
    }

    /// The bound is read on the parameter's own type, so autoref never lends a
    /// reference the extractor's answer.
    #[test]
    fn a_reference_to_an_extractor_answers_none() {
        assert_eq!(probe!(&'static CurrentTenant), None);
        assert_eq!(probe!(&'static Scoped<Tenant>), None);
    }
}
