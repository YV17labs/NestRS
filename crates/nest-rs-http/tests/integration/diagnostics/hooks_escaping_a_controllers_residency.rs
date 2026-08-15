//! The same escape, attempted on the host that shipped the defect this whole
//! refusal exists for: a `#[controller]` records `SINGLETON = false`, so
//! claiming otherwise collides with what the decorator already wrote instead of
//! filling a silence.

use nest_rs_core::{ProviderResidency, hooks};
use nest_rs_http::{controller, routes};

#[controller(path = "/things")]
struct ThingsController;

#[routes]
impl ThingsController {}

impl ProviderResidency for ThingsController {
    const SINGLETON: bool = true;
}

#[hooks]
impl ThingsController {
    #[on_module_init]
    async fn refuse_outside_development(&self) {}
}

fn main() {}
