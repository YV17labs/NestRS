//! A `#[scheduled]` trigger's method returns a `Result` the scheduler reads. One
//! answering `()` is refused naming that, in the sentence `#[process]` gives —
//! not with a type mismatch inside the registry entry the expansion writes.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s")]
    async fn tick(&self) {}
}

fn main() {}
