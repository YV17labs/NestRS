//! The way down, read across the crates that bound it: each transport's window,
//! the kernel's shutdown-hooks budget, the telemetry flush. No crate owns the
//! sum, so it is asserted here, where every one of them is reachable — as
//! dev-dependencies, never behind a feature, so the Definition-of-done run
//! compiles it.

use std::time::Duration;

use nest_rs_core::{SHUTDOWN_HOOKS_TIMEOUT, Transport};
use nest_rs_http::HttpTransport;
use nest_rs_opentelemetry::FLUSH_TIMEOUT;
use nest_rs_redis::RedisWorker;
use nest_rs_schedule::Scheduler;

/// `terminationGracePeriodSeconds`' default: Kubernetes' number, not ours.
const KUBERNETES_DEFAULT_GRACE: Duration = Duration::from_secs(30);

/// The way down the documentation states, to the millisecond: the longest
/// transport's stop, then the hooks' budget, then the flush. Pinned rather than
/// recomputed, so a constant that moves fails here and sends its author to every
/// page that quotes the figure.
const STATED_WAY_DOWN: Duration = Duration::from_millis(28_500);

/// Every transport a module can contribute, as it stands at its defaults. Each
/// states its own bound through the required [`Transport::stop_bound`], so a
/// row is the bound the transport answers for rather than a label beside it.
fn transports() -> [(&'static str, Box<dyn Transport>); 3] {
    [
        ("HttpTransport", Box::new(HttpTransport::default())),
        ("RedisWorker", Box::new(RedisWorker::default())),
        ("Scheduler", Box::new(Scheduler::default())),
    ]
}

/// Each transport's stop, then the shutdown hooks' budget, then the telemetry
/// flush, sums under the grace a Kubernetes pod is given by default between
/// `SIGTERM` and `SIGKILL`. Past it the kill lands first and says nothing,
/// taking the hooks and the flush with it. The runtime's teardown adds nothing:
/// `#[nest_rs::main]` holds it to what the hooks and the flush left of the
/// hooks' budget. Read off each transport's own bound and the two constants, so
/// moving any one of them past the sum fails here rather than in a rollout.
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
