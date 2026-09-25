//! One trigger repeated on one method is refused with the family's sentence,
//! counting every copy written rather than the first two.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s")]
    #[every("30s")]
    #[every("1m")]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
