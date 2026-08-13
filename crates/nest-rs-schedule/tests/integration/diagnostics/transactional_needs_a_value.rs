//! `transactional` written as a bare key. The refusal has to land on the key
//! itself — a trigger's arguments parse through `Punctuated<Meta, …>`, whose
//! failure is otherwise reported at the enclosing `#[scheduled]`, the *other*
//! half of the pair — and it has to say what the key expects, which a bare
//! `expected `=`` does not.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * *", transactional)]
    async fn nightly(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
