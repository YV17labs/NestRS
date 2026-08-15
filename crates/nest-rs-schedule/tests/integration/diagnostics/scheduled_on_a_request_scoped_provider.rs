//! A provider that *is* injectable and still cannot host these methods:
//! `scope = request` registers a factory, so `Container::get` outside a request
//! answers `None` and the work is skipped. The scope is known where
//! `#[injectable]` reads it, so the refusal belongs at compile time.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[scheduled]
impl PerRequest {
    #[every("60s")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
