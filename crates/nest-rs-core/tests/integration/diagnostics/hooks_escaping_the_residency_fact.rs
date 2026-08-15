//! The refusal cannot be waved through by taking the remedy it offers.
//!
//! `ProviderResidency` was a bare marker for exactly as long as it took to
//! audit: `impl Singleton for PerResolution {}` — one line, and the shape the
//! error names as the worst of them compiled clean. `#[injectable]` now records
//! the fact for every scope, so contradicting it is a coherence error rather
//! than a second opinion.

use nest_rs_core::{ProviderResidency, hooks, injectable};

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

impl ProviderResidency for PerResolution {
    const SINGLETON: bool = true;
}

#[hooks]
impl PerResolution {
    #[on_module_init]
    async fn init(&self) {}
}

fn main() {}
