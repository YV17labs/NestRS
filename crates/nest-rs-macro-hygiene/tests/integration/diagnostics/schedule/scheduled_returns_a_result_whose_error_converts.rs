//! A trigger's method returns `Result<(), E>`, read by its type, and the
//! scheduler fails the occurrence with its `E` converted into `anyhow::Error`:
//! through anyhow's `From` for a `std::error::Error + Send + Sync + 'static`,
//! or through a `From` of the error's own. An error converting neither way — no
//! `std::error::Error` and no `From`, or a box, `Send + Sync` or not, which has
//! neither (`#[on_event]` and `#[process]` take the `Send + Sync` one) — and an
//! answer that is no `Result<(), E>` are each one error at the return type, in
//! the framework's sentence, not a mismatch inside the registry entry the
//! expansion writes. An error borrowing the host converts by its type but not
//! for its lifetime, which no `#[diagnostic]` attribute reaches: rustc's own
//! lifetime error refuses it at `#[scheduled]`, naming the answer's type.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[derive(Debug)]
struct Declined;

#[derive(Debug)]
struct Refused<'a>(&'a Tasks);

impl std::fmt::Display for Refused<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the prune was refused")
    }
}

impl std::error::Error for Refused<'_> {}

#[injectable]
#[derive(Debug, Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s")]
    async fn sweep(&self) -> Result<(), Declined> {
        Err(Declined)
    }

    #[cron("0 0 * * * *")]
    async fn report(&self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    #[every("1m")]
    async fn audit(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    #[after("10s")]
    async fn count(&self) -> Result<u32, anyhow::Error> {
        Ok(0)
    }

    #[every("5m")]
    async fn prune<'a>(&'a self) -> Result<(), Refused<'a>> {
        Err(Refused(self))
    }
}

fn main() {}
