//! The link-time registry of every `#[config]` namespace the binary carries.
//!
//! `#[config(namespace = "…")]` submits one [`ConfigNamespace`] beside the
//! `Namespaced` impl it emits, so a binary knows every config namespace it
//! links from the first line of `main` — including the ones no module it
//! imports ever reads. That is the population the unclaimed-variable
//! diagnostic checks a variable's namespace against
//! ([`crate::unclaimed`]).
//!
//! **One type per namespace.** A namespace is read off the declaring file's
//! path exactly as the type's name is, so a variable under it names one type
//! and a reader finds that type from the variable. Two `#[config]` structs
//! declaring one namespace break that, and the report on unread keys with it —
//! it ran when the first of the two returned and filed the second one's
//! variables as read by nothing. So [`sole_declaration`] refuses the read of
//! either, naming both, whichever the binary reads first: the refusal depends
//! on what is linked, not on the order modules happen to load in.
//!
//! **The namespace, never the keys.** A config's keys are whatever its
//! hand-written `from_env` asks for, so they are knowable only where that
//! function runs — the claim registry's argument, recorded in
//! `service.rs`, and the reason nothing here tries to list them.

use crate::config::Namespaced;
use crate::error::ConfigError;

/// One `#[config]` namespace linked into the binary.
///
/// **Internal ABI** — submitted by the `#[config]` expansion, lockstep with
/// this crate; do not hand-construct. A config whose `Namespaced` impl is
/// written by hand files none, which costs it only the namespace-spelling half
/// of the diagnostic: the key half runs wherever the config is read.
#[doc(hidden)]
pub struct ConfigNamespace {
    namespace: &'static str,
    declaration: &'static str,
}

impl ConfigNamespace {
    /// The entry for `namespace`, exactly as `#[config(namespace = "…")]`
    /// declares it, on the struct `declaration` names — its module path and
    /// ident, the same string [`Namespaced::DECLARATION`](crate::Namespaced)
    /// carries.
    #[doc(hidden)]
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
/// The declarations come from the registry, so a namespace two `#[config]`
/// structs share is refused at the read of either; `C`'s own is added when its
/// `Namespaced` is hand-written, since such a type files no entry of its own and
/// still owns every variable under its namespace.
pub(crate) fn sole_declaration<C: Namespaced>() -> Result<(), ConfigError> {
    let mut declarations: Vec<&'static str> = nest_rs_core::inventory::iter::<ConfigNamespace>()
        .filter(|entry| entry.namespace == C::NAMESPACE)
        .map(|entry| entry.declaration)
        .collect();
    let own = if C::DECLARATION.is_empty() {
        std::any::type_name::<C>()
    } else {
        C::DECLARATION
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
