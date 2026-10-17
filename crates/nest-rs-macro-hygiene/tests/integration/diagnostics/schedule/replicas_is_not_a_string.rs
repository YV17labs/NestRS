//! `replicas` takes a string naming one of two behaviours; a bare word is not
//! one, and the refusal says what each value does rather than what type it
//! wanted.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", replicas = one)]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
