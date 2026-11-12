//! A deadline past a day bounds nothing an operator would wait for: the tick is
//! refused at the key, in the sentence `nest_rs::codegen::job` words for every
//! job decorator.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("1h", timeout = "25h")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
