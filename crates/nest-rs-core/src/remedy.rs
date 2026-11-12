//! The line a boot refusal names when a provider depends on a key no module the
//! app imports provides: filed by the crate owning the key's type
//! ([`DependencyRemedy`]), or carried by the type itself ([`CarriesRemedy`]) when
//! its key is one the app chooses — a resource generic over an
//! [`Instance`](crate::Instance).
//!
//! A macro reading tokens cannot tell a type carrying a remedy from any other,
//! so each declaration of a dependency — an `#[inject]` field, a handler's
//! parameter — asks [`RemedyProbe`] through autoref: [`RemedyProbeCarries`] is
//! implemented for `&RemedyProbe<T>` where `T` carries one,
//! [`RemedyProbeFallback`] for every `RemedyProbe<T>`, and a call on
//! `&&RemedyProbe<T>` takes the first whenever it applies. A type spelled with a
//! generic parameter resolves to the fallback.

use std::any::TypeId;
use std::borrow::Cow;
use std::marker::PhantomData;

/// The line to add when a provider depends on `key` and no module the app
/// imports provides it — submitted by the crate owning `key`'s type, so the
/// boot's [`MissingDependencyError`](crate::MissingDependencyError) names the
/// module that binds it instead of the kernel's generic remedy, as does
/// [`AccessGraphError`](crate::AccessGraphError) when no module the app reaches
/// lists the key. Text only: it mounts nothing, so it is no discovery to gate.
///
/// A port names its first-party binding for its trait object, and an adapter
/// the module opening its resource:
///
/// ```
/// use std::any::TypeId;
/// use std::sync::Arc;
///
/// pub trait Mailer: Send + Sync {}
///
/// nest_rs_core::inventory::submit! {
///     nest_rs_core::DependencyRemedy::new(
///         TypeId::of::<Arc<dyn Mailer>>,
///         "Import exactly one mailer binding — `acme::SmtpMailerModule` sends over SMTP.",
///     )
/// }
/// ```
///
/// One key takes one remedy, its owning crate's: filed here for a key the crate
/// can name, carried by the type ([`CarriesRemedy`]) for one the app chooses.
pub struct DependencyRemedy {
    key: fn() -> TypeId,
    remedy: &'static str,
}

impl DependencyRemedy {
    /// `remedy` for the container key `key` — `TypeId::of::<Arc<dyn Port>>`
    /// for a trait object, the type itself otherwise.
    pub const fn new(key: fn() -> TypeId, remedy: &'static str) -> Self {
        Self { key, remedy }
    }
}

inventory::collect!(DependencyRemedy);

/// The line to add for `key`, read where a refusal is written: the remedy its
/// type carries where the provider declares it (`carried`), else the one the
/// crate owning it files.
pub(crate) fn remedy_for(
    key: TypeId,
    carried: fn() -> Vec<InjectedRemedy>,
) -> Option<Cow<'static, str>> {
    carried()
        .into_iter()
        .find(|(declared, _)| *declared == key)
        .map(|(_, remedy)| remedy())
        .or_else(|| {
            inventory::iter::<DependencyRemedy>()
                .find(|entry| (entry.key)() == key)
                .map(|entry| Cow::Borrowed(entry.remedy))
        })
}

/// A key whose type names the line to add when a provider depends on it and no
/// module the app imports provides it — what a [`DependencyRemedy`] files, for
/// a key its crate cannot name: a type generic over what the app chooses, an
/// [`Instance`](crate::Instance) above all, whose remedy names the app's own
/// instance.
///
/// Read where the dependency is declared, an `#[inject]` field or a handler's
/// parameter, and said by the boot's
/// [`MissingDependencyError`](crate::MissingDependencyError) and
/// [`AccessGraphError`](crate::AccessGraphError) in place of the kernel's
/// sentence. A handler parameter is asked through its own type, so an
/// extractor resolving a provider that carries a remedy forwards it, as
/// [`Scoped<T>`](crate::Scoped) carries `T`'s
/// ([`ResolvesProvider`](crate::ResolvesProvider)).
///
/// ```
/// use std::borrow::Cow;
/// use std::marker::PhantomData;
///
/// use nest_rs_core::{CarriesRemedy, Instance, Unnamed};
///
/// pub struct Pool<I = Unnamed>(PhantomData<fn() -> I>);
///
/// impl<I: Instance> CarriesRemedy for Pool<I> {
///     fn remedy() -> Cow<'static, str> {
///         match I::NAME {
///             "" => "Import `acme::PoolModule`, which opens the default pool.".into(),
///             name => format!("Import `acme::PoolModule` for the `{name}` instance.").into(),
///         }
///     }
/// }
/// ```
pub trait CarriesRemedy {
    /// The line to add, said in place of the kernel's.
    fn remedy() -> Cow<'static, str>;
}

/// A key a provider depends on, beside the remedy its type carries
/// ([`CarriesRemedy::remedy`]) — one entry of
/// [`Discoverable::injected_remedies`](crate::Discoverable::injected_remedies).
pub type InjectedRemedy = (TypeId, fn() -> Cow<'static, str>);

/// A declared dependency's type, held so the compiler can be asked whether it
/// carries a remedy.
pub struct RemedyProbe<T: ?Sized>(PhantomData<fn() -> *const T>);

impl<T: ?Sized> RemedyProbe<T> {
    /// The probe for the type `T`.
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: ?Sized> Default for RemedyProbe<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`RemedyProbe`] answers for a type carrying a remedy.
pub trait RemedyProbeCarries {
    /// The remedy the type carries.
    fn carried_remedy(&self) -> Option<fn() -> Cow<'static, str>>;
}

impl<T: ?Sized + CarriesRemedy> RemedyProbeCarries for &RemedyProbe<T> {
    fn carried_remedy(&self) -> Option<fn() -> Cow<'static, str>> {
        Some(T::remedy)
    }
}

/// What [`RemedyProbe`] answers for every other type.
pub trait RemedyProbeFallback {
    /// None: the type carries no remedy.
    fn carried_remedy(&self) -> Option<fn() -> Cow<'static, str>>;
}

impl<T: ?Sized> RemedyProbeFallback for RemedyProbe<T> {
    fn carried_remedy(&self) -> Option<fn() -> Cow<'static, str>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{CarriesRemedy, RemedyProbe, RemedyProbeCarries as _, RemedyProbeFallback as _};

    struct Opened<T>(T);

    impl<T> CarriesRemedy for Opened<T> {
        fn remedy() -> Cow<'static, str> {
            format!(
                "Import the module opening `{}`.",
                std::any::type_name::<T>()
            )
            .into()
        }
    }

    trait Port {}

    impl CarriesRemedy for dyn Port {
        fn remedy() -> Cow<'static, str> {
            "Import the port's binding.".into()
        }
    }

    /// The expansion's call, written once so every test asks at the same
    /// reference depth.
    macro_rules! probe {
        ($ty:ty) => {
            (&&RemedyProbe::<$ty>::new())
                .carried_remedy()
                .map(|remedy| remedy())
        };
    }

    #[test]
    fn a_type_carrying_a_remedy_answers_it() {
        assert_eq!(
            probe!(Opened<u8>),
            Some(Cow::Owned("Import the module opening `u8`.".to_owned()))
        );
        assert_eq!(
            probe!(dyn Port),
            Some(Cow::Borrowed("Import the port's binding."))
        );
    }

    #[test]
    fn a_type_carrying_none_answers_none() {
        assert_eq!(probe!(u32), None);
        assert_eq!(probe!(str), None);
        assert_eq!(probe!(dyn Send), None);
    }

    /// Nothing proves the bound inside a generic function, whatever the type
    /// it is called with: the dependency resolves to the fallback.
    #[test]
    fn a_type_spelled_with_a_generic_parameter_answers_none() {
        fn generic<T>() -> Option<Cow<'static, str>> {
            probe!(T)
        }
        assert_eq!(generic::<Opened<u8>>(), None);
    }

    /// The bound is read on the type itself, so autoref never lends a
    /// reference the type's answer.
    #[test]
    fn a_reference_to_a_type_carrying_a_remedy_answers_none() {
        assert_eq!(probe!(&'static Opened<u8>), None);
    }
}
