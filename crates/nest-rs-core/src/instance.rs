//! Named instances: a resource an app opens more than once is told apart by a
//! marker type, so each instance is a type of its own to the container.

use std::any::TypeId;
use std::borrow::Borrow;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;

/// An instance of a resource an app opens more than once; [`Unnamed`] is the
/// default one.
///
/// A marker is declared with [`#[instance]`](macro@crate::instance), which
/// reads its name off the ident and files it for the twin check: the trait is
/// sealed, so no hand-written impl chooses a name or shares the default's key.
pub trait Instance: InstanceSeal + Send + Sync + 'static {
    /// The segment this instance inserts after its config's vendor —
    /// `cache` in `<PREFIX>_REDIS__CACHE__URL` — and the `instance` its lines
    /// carry; empty for [`Unnamed`], which inserts none.
    const NAME: &'static str;

    /// What the container holds a `T` under for this instance: `T` itself for
    /// [`Unnamed`], so a seed of `T` keeps working, and [`Instanced<Self, T>`]
    /// for a named one, so two instances never share a key.
    type Of<T: Send + Sync + 'static>: Borrow<T> + From<T> + Send + Sync + 'static;
}

/// What `#[instance]` implements beside [`Instance`], and core for [`Unnamed`],
/// so a marker exists only through the decorator. Not API.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an instance marker",
    label = "declare it with `#[instance]`",
    note = "an instance's name is read off its marker's ident, never chosen: `#[instance] pub struct Cache;`"
)]
pub trait InstanceSeal {}

/// The default instance: it adds no segment to a namespace, so an app opening a
/// resource once names no instance and reads its variables as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Unnamed;

#[diagnostic::do_not_recommend]
impl InstanceSeal for Unnamed {}

impl Instance for Unnamed {
    const NAME: &'static str = "";
    type Of<T: Send + Sync + 'static> = T;
}

/// A value of `T` belonging to the instance `I` — how the container holds a
/// named instance's config, or any value an app holds once per instance.
///
/// ```
/// use nest_rs_core::{Instanced, instance};
///
/// #[instance]
/// pub struct Cache;
///
/// let url: Instanced<Cache, String> = String::from("rediss://cache:6379").into();
/// assert_eq!(url.as_str(), "rediss://cache:6379");
/// ```
pub struct Instanced<I, T> {
    value: T,
    instance: PhantomData<fn() -> I>,
}

impl<I, T> Instanced<I, T> {
    /// The value, out of its instance.
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<I, T> From<T> for Instanced<I, T> {
    fn from(value: T) -> Self {
        Self {
            value,
            instance: PhantomData,
        }
    }
}

impl<I, T> Deref for Instanced<I, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<I, T> Borrow<T> for Instanced<I, T> {
    fn borrow(&self) -> &T {
        &self.value
    }
}

impl<I, T: Clone> Clone for Instanced<I, T> {
    fn clone(&self) -> Self {
        self.value.clone().into()
    }
}

impl<I: Instance, T: fmt::Debug> fmt::Debug for Instanced<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instanced")
            .field("instance", &I::NAME)
            .field("value", &self.value)
            .finish()
    }
}

/// One `#[instance]` marker linked into the binary, submitted by its
/// expansion, so two markers reading one name are refused at first use.
pub struct InstanceDeclaration {
    name: &'static str,
    declaration: &'static str,
    marker: fn() -> TypeId,
}

impl InstanceDeclaration {
    /// The entry for the marker `declaration` names — its module path and
    /// ident — whose instance reads `name`; `marker` tells it from a twin of
    /// the same path, a marker declared inside a function.
    pub const fn new(
        name: &'static str,
        declaration: &'static str,
        marker: fn() -> TypeId,
    ) -> Self {
        Self {
            name,
            declaration,
            marker,
        }
    }

    /// Every marker linked into the binary, `I` aside, whose instance reads
    /// `I`'s name, by its module path and ident.
    pub fn others<I: Instance>() -> impl Iterator<Item = &'static str> {
        inventory::iter::<Self>()
            .filter(|entry| entry.name == I::NAME && (entry.marker)() != TypeId::of::<I>())
            .map(|entry| entry.declaration)
    }
}

inventory::collect!(InstanceDeclaration);

/// `module` as an app imports it for the instance `I`: its bare path for the
/// default instance, typed with the marker for a named one —
/// `nest_rs::redis::RedisModule::<Cache>` — the import a remedy names.
pub fn typed_import<I: Instance>(module: &str) -> String {
    match I::NAME {
        "" => module.to_owned(),
        _ => format!("{module}::<{}>", crate::type_name::short_type_name::<I>()),
    }
}

/// The instance `I` as a remedy's sentence names it: the default one, or the
/// one its marker's name reads.
pub fn instance_phrase<I: Instance>() -> std::borrow::Cow<'static, str> {
    match I::NAME {
        "" => std::borrow::Cow::Borrowed("the default instance"),
        name => std::borrow::Cow::Owned(format!("the `{name}` instance")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remedy_names_the_default_instance_s_import_bare_and_a_named_one_s_typed() {
        assert_eq!(
            typed_import::<Unnamed>("nest_rs::redis::RedisModule"),
            "nest_rs::redis::RedisModule"
        );
        assert_eq!(instance_phrase::<Unnamed>(), "the default instance");
        assert_eq!(
            typed_import::<Cache>("nest_rs::redis::RedisModule"),
            "nest_rs::redis::RedisModule::<Cache>"
        );
        assert_eq!(instance_phrase::<Cache>(), "the `cache` instance");
    }

    struct Cache;

    impl InstanceSeal for Cache {}

    impl Instance for Cache {
        const NAME: &'static str = "cache";
        type Of<T: Send + Sync + 'static> = Instanced<Self, T>;
    }

    fn held<I: Instance>(value: u16) -> I::Of<u16> {
        value.into()
    }

    #[test]
    fn the_default_instance_holds_the_value_itself_and_a_named_one_wraps_it() {
        let unnamed: u16 = held::<Unnamed>(6379);
        let cache: Instanced<Cache, u16> = held::<Cache>(6380);
        assert_eq!(unnamed, 6379);
        assert_eq!(*cache, 6380);
        assert_eq!(*Borrow::<u16>::borrow(&cache), 6380);
        assert_eq!(cache.clone().into_inner(), 6380);
    }

    #[test]
    fn a_named_value_says_its_instance_when_debugged() {
        let cache: Instanced<Cache, u16> = 6380.into();
        assert_eq!(
            format!("{cache:?}"),
            r#"Instanced { instance: "cache", value: 6380 }"#
        );
    }
}
