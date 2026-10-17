//! Boot-time fail-secure checks the HTTP transport runs before mounting
//! anything, for what matters only when it serves: a controller's or a
//! gateway's declared guards, an endpoint's own declarations. A check every app
//! owes, whatever it serves, runs on the kernel's wiring step instead.

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
