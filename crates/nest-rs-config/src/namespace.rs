//! The link-time registry of every `#[config]` namespace the binary carries.
//!
//! `#[config(namespace = "…")]` submits one [`ConfigNamespace`] beside the
//! `Namespaced` impl it emits, so a binary knows every config namespace it
//! links from the first line of `main` — including the ones no module it
//! imports ever reads. That is the population the unclaimed-variable
//! diagnostic checks a variable's namespace against
//! ([`crate::unclaimed`]).
//!
//! **The namespace, never the keys.** A config's keys are whatever its
//! hand-written `from_env` asks for, so they are knowable only where that
//! function runs — the claim registry's argument, recorded in
//! `service.rs`, and the reason nothing here tries to list them.

/// One `#[config]` namespace linked into the binary.
///
/// **Internal ABI** — submitted by the `#[config]` expansion, lockstep with
/// this crate; do not hand-construct. A config whose `Namespaced` impl is
/// written by hand files none, which costs it only the namespace-spelling half
/// of the diagnostic: the key half runs wherever the config is read.
#[doc(hidden)]
pub struct ConfigNamespace {
    namespace: &'static str,
}

impl ConfigNamespace {
    /// The entry for `namespace`, exactly as `#[config(namespace = "…")]`
    /// declares it.
    #[doc(hidden)]
    pub const fn new(namespace: &'static str) -> Self {
        Self { namespace }
    }
}

nest_rs_core::inventory::collect!(ConfigNamespace);

/// Every namespace a `#[config]` in this binary declares, deduplicated — a
/// namespace two structs share is one namespace.
pub(crate) fn linked() -> Vec<&'static str> {
    let mut namespaces: Vec<&'static str> = nest_rs_core::inventory::iter::<ConfigNamespace>()
        .map(|entry| entry.namespace)
        .collect();
    namespaces.sort_unstable();
    namespaces.dedup();
    namespaces
}
