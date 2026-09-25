//! The shared duration grammar, refused at the literal: a number with no unit
//! could mean seconds or milliseconds, and guessing is how a job ticks a
//! thousand times too often.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
