//! Boot-time fail-secure checks the HTTP transport runs before mounting
//! anything: a global spec whose provider was never registered would otherwise
//! be dropped silently, and every route would lose that guard.

use nest_rs_core::Container;

type CheckFn = Box<dyn Fn(&Container) -> Result<(), String> + Send + Sync>;

/// A boot-time check the HTTP transport runs at the start of `configure`.
/// Returning `Err(message)` aborts boot with that message.
pub struct HttpBootCheck(CheckFn);

impl HttpBootCheck {
    /// Register a check to run at `configure` — `Err(message)` aborts boot.
    pub fn new<F>(check: F) -> Self
    where
        F: Fn(&Container) -> Result<(), String> + Send + Sync + 'static,
    {
        Self(Box::new(check))
    }

    /// Run the check against the live container.
    pub fn run(&self, container: &Container) -> Result<(), String> {
        (self.0)(container)
    }
}

/// Marker provided by `use_guards_global` when at least one global guard is
/// registered; the transport reads it to refuse an unguardable `mount(...)`.
pub struct GlobalGuardsActive;
