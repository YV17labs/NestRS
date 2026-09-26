//! `queue` names the `#[queue]` marker a method drains, and a value that is not a
//! type is refused at itself, opening with the decorator and the key — not with
//! syn's list of the tokens a type may start with.

use nest_rs_core::injectable;
use nest_rs_queue::processor;

#[injectable]
#[derive(Default)]
struct Demo;

#[processor]
impl Demo {
    #[process(queue = 42)]
    async fn handle(&self, _job: String) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
