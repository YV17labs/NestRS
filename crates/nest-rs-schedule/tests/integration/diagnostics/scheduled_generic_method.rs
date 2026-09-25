//! A `#[cron]` method takes no type parameter: the expansion calls it with only
//! what its caller carries, so nothing supplies the type — refused naming the
//! parameter, not with rustc's `type annotations needed` at the decorator.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * * *")]
    async fn tick<T: Default>(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
