//! `#[every]` reads its period through the one duration grammar, and a value that
//! is not a string literal gets that grammar's sentence — not syn's bare
//! `expected string literal`, which named neither the decorator nor the grammar.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every(30)]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
