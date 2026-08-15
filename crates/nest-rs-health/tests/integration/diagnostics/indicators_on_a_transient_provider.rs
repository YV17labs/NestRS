//! The worst of the shapes: `Container::get` on a `scope = transient`
//! provider **builds a throwaway**, so the methods run against an instance
//! nobody else holds and their effects are dropped — no skip, no warning, no
//! symptom.

use nest_rs_core::injectable;
use nest_rs_health::indicators;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

#[indicators]
impl PerResolution {
    #[readiness]
    async fn ready(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
