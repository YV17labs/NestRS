//! The same key, the same sentence, on the other half of the family — which is
//! the point of wording it in `nest_rs_codegen::job` rather than at each site.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", transactional = "no")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
