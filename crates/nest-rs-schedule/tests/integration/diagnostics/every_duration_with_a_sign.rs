//! The shared duration grammar takes a whole number, digits only. A leading `+`
//! is what Rust's integer parser accepts and the grammar does not state, so it
//! is refused at the literal rather than read as five seconds.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("+5s")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
