//! The namespaces a binary reads: every `#[config]` namespace it links, and
//! the one a named instance derives from its config's.
//!
//! `#[config(namespace = "…")]` submits one [`ConfigNamespace`] beside the
//! `Namespaced` impl it emits, so a binary knows every config namespace it
//! links, read or not — the population [`crate::unclaimed`] checks against.
//! One type per namespace: [`sole_declaration`] refuses the read of either of
//! two, whichever the binary reads first, and [`sole_instance`] an instance
//! whose namespace another declaration holds.

use std::borrow::Cow;

use nest_rs_core::__private::InstanceDeclaration;
use nest_rs_core::Instance;

use crate::config::Namespaced;
use crate::error::ConfigError;

/// One `#[config]` namespace linked into the binary, submitted by the
/// `#[config]` expansion.
pub struct ConfigNamespace {
    namespace: &'static str,
    declaration: &'static str,
}

impl ConfigNamespace {
    /// The entry for `namespace`, exactly as `#[config(namespace = "…")]`
    /// declares it, on the struct `declaration` names — its module path and
    /// ident, the same string `Namespaced::__DECLARATION` carries.
    pub const fn new(namespace: &'static str, declaration: &'static str) -> Self {
        Self {
            namespace,
            declaration,
        }
    }
}

nest_rs_core::inventory::collect!(ConfigNamespace);

/// Refuse to read `C` when its namespace is declared by another type too.
///
/// `C`'s own declaration is added when its `Namespaced` is hand-written, since
/// such a type files no registry entry.
pub(crate) fn sole_declaration<C: Namespaced>() -> Result<(), ConfigError> {
    let mut declarations: Vec<&'static str> = nest_rs_core::inventory::iter::<ConfigNamespace>()
        .filter(|entry| entry.namespace == C::NAMESPACE)
        .map(|entry| entry.declaration)
        .collect();
    let own = if C::__DECLARATION.is_empty() {
        std::any::type_name::<C>()
    } else {
        C::__DECLARATION
    };
    declarations.push(own);
    declarations.sort_unstable();
    declarations.dedup();
    if declarations.len() > 1 {
        return Err(ConfigError::SharedNamespace {
            namespace: C::NAMESPACE,
            declarations,
        });
    }
    Ok(())
}

/// The namespace `I`'s instance of a config declaring `namespace` reads: the
/// instance's name inserted after the first segment — vendor, instance, port.
///
/// The default instance reads `namespace` itself.
///
/// ```
/// use nest_rs_config::instance_namespace;
/// use nest_rs_core::{Unnamed, instance};
///
/// #[instance]
/// pub struct Cache;
///
/// assert_eq!(instance_namespace::<Cache>("redis"), "redis__cache");
/// assert_eq!(instance_namespace::<Cache>("redis__queue"), "redis__cache__queue");
/// assert_eq!(instance_namespace::<Unnamed>("redis__queue"), "redis__queue");
/// ```
pub fn instance_namespace<I: Instance>(namespace: &'static str) -> Cow<'static, str> {
    if I::NAME.is_empty() {
        return Cow::Borrowed(namespace);
    }
    Cow::Owned(match namespace.split_once("__") {
        Some((vendor, port)) => format!("{vendor}__{}__{port}", I::NAME),
        None => format!("{namespace}__{}", I::NAME),
    })
}

/// Refuse to read a config for the named instance `I` under `namespace`, its
/// derived one, when a linked `#[config]` declares that namespace or another
/// marker reads `I`'s name.
pub(crate) fn sole_instance<I: Instance>(namespace: &str) -> Result<(), ConfigError> {
    if I::NAME.is_empty() {
        return Ok(());
    }
    let instance = std::any::type_name::<I>();
    let mut claims: Vec<&'static str> = nest_rs_core::inventory::iter::<ConfigNamespace>()
        .filter(|entry| entry.namespace == namespace)
        .map(|entry| entry.declaration)
        .chain(InstanceDeclaration::others::<I>())
        .collect();
    if claims.is_empty() {
        return Ok(());
    }
    claims.sort_unstable();
    claims.dedup();
    Err(ConfigError::SharedInstance {
        instance,
        namespace: namespace.to_owned(),
        claims,
    })
}

/// Every namespace a `#[config]` in this binary declares, deduplicated.
pub(crate) fn linked() -> Vec<&'static str> {
    let mut namespaces: Vec<&'static str> = nest_rs_core::inventory::iter::<ConfigNamespace>()
        .map(|entry| entry.namespace)
        .collect();
    namespaces.sort_unstable();
    namespaces.dedup();
    namespaces
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{Unnamed, instance};

    use super::*;

    #[instance]
    struct Cache;

    #[test]
    fn a_named_instance_inserts_its_name_after_the_vendor() {
        assert_eq!(instance_namespace::<Cache>("redis"), "redis__cache");
        assert_eq!(
            instance_namespace::<Cache>("redis__queue"),
            "redis__cache__queue"
        );
        assert_eq!(
            instance_namespace::<Cache>("social__github__app"),
            "social__cache__github__app"
        );
    }

    #[test]
    fn the_default_instance_reads_its_config_s_namespace_borrowed() {
        assert!(matches!(
            instance_namespace::<Unnamed>("redis__queue"),
            Cow::Borrowed("redis__queue")
        ));
    }
}
