//! The pair's third wrong shape: right item, wrong host. `#[hooks]` gates on
//! `Container::get::<Host>()` at boot, so a host nothing registers — a plain
//! struct, or an edge host like a `#[controller]` — would have its methods
//! silently skipped behind a `warn`. The refusal reads `ProviderResidency`,
//! the fact every provider-building decorator records.

use nest_rs_core::hooks;

struct Plain;

#[hooks]
impl Plain {
    #[on_module_init]
    async fn init(&self) {}
}

fn main() {}
