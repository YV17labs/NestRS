//! The worst of the shapes: `Container::get` on a `scope = transient`
//! provider **builds a throwaway**, so the methods run against an instance
//! nobody else holds and their effects are dropped — no skip, no warning, no
//! symptom.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

#[scheduled]
impl PerResolution {
    #[every("60s")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
