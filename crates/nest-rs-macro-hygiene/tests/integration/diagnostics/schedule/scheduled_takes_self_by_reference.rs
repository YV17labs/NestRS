//! A scheduled method borrows its host: `&mut self` is refused with the fact the
//! provider-hosted orchestrators share, quoting the receiver written — not with
//! rustc's `cannot borrow data in an Arc as mutable` about the expansion's call.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * * *")]
    async fn tick(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
