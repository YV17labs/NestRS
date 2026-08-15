//! The composition this bound exists for: a `#[controller]` registers metadata,
//! never an instance, so a `#[hooks]` block on it used to be *skipped* at boot
//! behind a `warn` — the routes mounted, the app served, and the lifecycle
//! method never ran. Refused at compile time: `#[controller]` records
//! `ProviderResidency::SINGLETON = false`, and `#[hooks]` reads it.

use nest_rs_core::hooks;
use nest_rs_http::{controller, routes};

#[controller(path = "/things")]
struct ThingsController;

#[routes]
impl ThingsController {}

#[hooks]
impl ThingsController {
    #[on_module_init]
    async fn refuse_outside_development(&self) {}
}

fn main() {}
