//! Covers `src/unit.rs` from the site's side: the declaration each trigger
//! emits, naming its unit and carrying the impl block's and its own `#[meta]`.

use nest_rs_core::{Posture, injectable};
use nest_rs_schedule::scheduled;

#[derive(Debug, PartialEq)]
struct Audit(&'static str);

#[derive(Debug, PartialEq)]
struct Cost(u32);

/// Declared only on a site no test reads, so built by its declaration alone.
#[deny(dead_code)]
struct Unread;

#[injectable]
#[derive(Default)]
struct MeteredTasks;

#[scheduled]
#[meta(Audit("tasks"))]
#[meta(Cost(1))]
impl MeteredTasks {
    #[every("1h")]
    #[meta(Cost(5))]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("1h", replicas = "one", key = "metered::pinned")]
    async fn pinned(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("1h")]
    #[meta(Unread)]
    async fn idle(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn a_trigger_declares_its_unit_address_and_metadata() {
    let handler = MeteredTasks::__NESTRS_HANDLER_SWEEP;
    assert_eq!(handler.unit(), nest_rs_schedule::unit::TICK);
    assert_eq!(handler.host_name(), "MeteredTasks");
    assert_eq!(handler.method(), "sweep");
    assert_eq!(handler.address(), "MeteredTasks::sweep");
    assert_eq!(handler.posture(), Posture::System);
    let reflector = handler.reflector();
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(5)));
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("tasks")));

    let pinned = MeteredTasks::__NESTRS_HANDLER_PINNED;
    assert_eq!(pinned.address(), "metered::pinned", "the key the job pins");
    assert_eq!(pinned.reflector().get::<Cost>(), Some(&Cost(1)));
}
