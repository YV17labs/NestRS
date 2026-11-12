//! `transactional` written as a bare key, at every trigger — it is the one key
//! each of them takes. The refusal has to land on the key itself — a trigger's
//! arguments parse through `Punctuated<Meta, …>`, whose failure is otherwise
//! reported at the enclosing `#[scheduled]`, the *other* half of the pair — and
//! it has to say what the key expects, which a bare `expected `=`` does not. One
//! method per trigger; the refusals of one host arrive together.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * *", transactional)]
    async fn nightly(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", transactional)]
    async fn poll(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", transactional)]
    async fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
