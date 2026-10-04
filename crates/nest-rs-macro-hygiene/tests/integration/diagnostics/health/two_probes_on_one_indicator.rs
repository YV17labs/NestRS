//! The health member of the one-role-per-method family. It shares
//! `nest_rs::codegen::one_role_per_method` with `#[hooks]`, `#[scheduled]`,
//! `#[operations]` and `#[tools]`, so the five cannot drift — four used to word
//! it themselves and the fifth said nothing.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

#[injectable]
#[derive(Default)]
struct AppHealth;

#[indicators]
impl AppHealth {
    #[liveness]
    #[readiness]
    async fn db(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
