//! The way down, read across the crates that bound it: each transport's window,
//! the kernel's shutdown-hooks budget, the telemetry flush. No crate owns the
//! sum, so it is asserted here, where every one of them is reachable.

use std::time::Duration;

use nest_rs_core::{SHUTDOWN_HOOKS_TIMEOUT, Transport};
use nest_rs_http::HttpTransport;
use nest_rs_opentelemetry::FLUSH_TIMEOUT;
use nest_rs_queue::QueueWorker;
use nest_rs_schedule::Scheduler;

/// `terminationGracePeriodSeconds`' default: Kubernetes' number, not ours.
const KUBERNETES_DEFAULT_GRACE: Duration = Duration::from_secs(30);

/// The way down the documentation states, to the millisecond: the longest
/// transport's stop, then the hooks' budget, then the flush. Pinned, so a moved
/// constant sends its author to every page that quotes the figure.
const STATED_WAY_DOWN: Duration = Duration::from_millis(28_500);

/// Every transport a module can contribute, at its defaults.
fn transports() -> [(&'static str, Box<dyn Transport>); 3] {
    [
        ("HttpTransport", Box::new(HttpTransport::default())),
        ("QueueWorker", Box::new(QueueWorker::default())),
        ("Scheduler", Box::new(Scheduler::default())),
    ]
}

/// The runtime's teardown adds nothing: `#[nest_rs::main]` holds it to what the
/// hooks and the flush left of the hooks' budget.
#[test]
fn the_default_shutdown_steps_sum_under_a_kubernetes_grace_period() {
    let after = SHUTDOWN_HOOKS_TIMEOUT + FLUSH_TIMEOUT;
    for (transport, serving) in transports() {
        let way_down = serving.stop_bound() + after;
        assert!(
            way_down < KUBERNETES_DEFAULT_GRACE,
            "an app serving {transport} takes {way_down:?} to stop by default, which a \
             {KUBERNETES_DEFAULT_GRACE:?} grace period cuts short",
        );
    }
    let longest = transports()
        .into_iter()
        .map(|(_, transport)| transport.stop_bound())
        .max()
        .unwrap_or_default();
    assert_eq!(
        longest + after,
        STATED_WAY_DOWN,
        "the documented way down no longer matches the constants; correct every page that \
         states it",
    );
}
