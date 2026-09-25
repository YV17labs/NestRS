//! One probe repeated on one method is refused saying what was written — the
//! same probe twice — rather than naming a second probe nobody wrote.

use nest_rs_core::injectable;
use nest_rs_health::indicators;

#[injectable]
#[derive(Default)]
struct AppHealth;

#[indicators]
impl AppHealth {
    #[readiness]
    #[readiness]
    async fn db(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
