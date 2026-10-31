//! A guard bound where it does not declare that it checks is refused at the
//! binding.
//!
//! The guard writes its decision, but no edge: without the `GuardFor<HttpView>`
//! bound it would run beside a verb on a view its decision was never written
//! for.

use nest_rs::core::{Layer, UnitContext, async_trait, injectable};
use nest_rs::guards::{Denial, Guard};
use nest_rs::http::{controller, routes};

#[injectable]
#[derive(Default)]
struct UnattestedGuard;

impl Layer for UnattestedGuard {}

#[async_trait]
impl Guard for UnattestedGuard {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

#[controller(path = "/demo")]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/ping")]
    #[use_guards(UnattestedGuard)]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn main() {}
