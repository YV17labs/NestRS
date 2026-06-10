//! Boot-time fail-secure checks the HTTP transport runs before mounting
//! anything.
//!
//! The global layer pools are declared as *specs* (`use_guards_global` and
//! friends) and resolved against the live container at configure time. A
//! spec whose provider was never registered would otherwise resolve to
//! `None` and be dropped **silently** — for a guard that means every route
//! quietly loses its fail-secure net. Each `use_*_global` builder therefore
//! attaches an [`HttpBootCheck`] that re-resolves its specs at configure
//! time and fails boot with the offending type names, the same posture as
//! the access graph (a wiring error is a boot error, never a runtime
//! surprise).

use nest_rs_core::Container;

type CheckFn = Box<dyn Fn(&Container) -> Result<(), String> + Send + Sync>;

/// A boot-time check the HTTP transport runs at the start of `configure`.
/// Returning `Err(message)` aborts boot with that message.
pub struct HttpBootCheck(CheckFn);

impl HttpBootCheck {
    pub fn new<F>(check: F) -> Self
    where
        F: Fn(&Container) -> Result<(), String> + Send + Sync + 'static,
    {
        Self(Box::new(check))
    }

    pub fn run(&self, container: &Container) -> Result<(), String> {
        (self.0)(container)
    }
}

/// Marker provided by `use_guards_global` when at least one global guard is
/// registered. The transport reads it (it cannot see the `Guard` trait or
/// `GuardSpecs`) to decide whether an unguardable endpoint — an imperative
/// `mount(...)` the transport can neither shape nor introspect — is a
/// fail-secure violation.
pub struct GlobalGuardsActive;
