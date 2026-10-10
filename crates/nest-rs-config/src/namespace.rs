//! The link-time registry of every `#[config]` namespace the binary carries.
//!
//! `#[config(namespace = "…")]` submits one [`ConfigNamespace`] beside the
//! `Namespaced` impl it emits, so a binary knows every config namespace it
//! links, read or not — the population [`crate::unclaimed`] checks against.
//! One type per namespace: [`sole_declaration`] refuses the read of either of
//! two, whichever the binary reads first.

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

/// Every namespace a `#[config]` in this binary declares, deduplicated.
pub(crate) fn linked() -> Vec<&'static str> {
    let mut namespaces: Vec<&'static str> = nest_rs_core::inventory::iter::<ConfigNamespace>()
        .map(|entry| entry.namespace)
        .collect();
    namespaces.sort_unstable();
    namespaces.dedup();
    namespaces
}
