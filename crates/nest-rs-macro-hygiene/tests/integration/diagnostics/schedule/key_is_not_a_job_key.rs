//! A key is one or more levels joined by `::`, each non-empty and free of `:`,
//! whitespace and control characters — the levels of the key a backend claims
//! under. Anything else is refused at the literal, at both triggers.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", replicas = "one", key = "billing:Tasks:tick")]
    async fn single_colons(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", replicas = "one", key = "billing::")]
    async fn empty_level(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 0 * * * *", replicas = "one", key = "billing::close day")]
    async fn whitespace(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
