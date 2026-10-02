//! The way down, read across the crates that bound it: each transport's window,
//! the kernel's shutdown-hooks budget, the telemetry flush. No crate owns the
//! sum, so it is asserted here, where every one of them is reachable — as
//! dev-dependencies, never behind a feature, so the Definition-of-done run
//! compiles it.

use std::time::Duration;

use nest_rs_core::{SHUTDOWN_HOOKS_TIMEOUT, SHUTDOWN_SETTLE_TIMEOUT};
use nest_rs_http::HttpConfig;
use nest_rs_opentelemetry::FLUSH_TIMEOUT;
use nest_rs_redis::RedisWorkerConfig;
use nest_rs_schedule::Scheduler;

/// `terminationGracePeriodSeconds`' default: Kubernetes' number, not ours.
const KUBERNETES_DEFAULT_GRACE: Duration = Duration::from_secs(30);

/// The way down the documentation states, to the millisecond: the longest
/// transport's stop, then the hooks' budget, then the flush. Pinned rather than
/// recomputed, so a constant that moves fails here and sends its author to every
/// page that quotes the figure.
const STATED_WAY_DOWN: Duration = Duration::from_millis(28_500);

/// Every transport a module can contribute, with the bound on its own stop from
/// the signal to its `serve` returning, at its defaults. One row per
/// `impl Transport` in the framework — the conformance suite holds the list to
/// the source.
fn transports() -> [(&'static str, Duration); 3] {
    [
        // The window, then the settle `serve` spends stopping what its
        // self-mounts ran off their connections.
        (
            "HttpTransport",
            HttpConfig::default().shutdown_timeout + SHUTDOWN_SETTLE_TIMEOUT,
        ),
        // The drain window holds its own reserve for handing interrupted jobs
        // back, so it is the whole of the worker's stop.
        ("RedisWorker", RedisWorkerConfig::default().shutdown_timeout),
        // A running tick gets its bound, then the settle once it is stopped.
        (
            "Scheduler",
            Scheduler::SHUTDOWN_TIMEOUT + SHUTDOWN_SETTLE_TIMEOUT,
        ),
    ]
}

/// Each transport's stop, then the shutdown hooks' budget, then the telemetry
/// flush, sums under the grace a Kubernetes pod is given by default between
/// `SIGTERM` and `SIGKILL`. Past it the kill lands first and says nothing,
/// taking the hooks and the flush with it. The runtime's teardown adds nothing:
/// `#[nest_rs::main]` holds it to what the hooks and the flush left of the
/// hooks' budget. Read off the constants, so moving any one of them past the sum
/// fails here rather than in a rollout.
#[test]
fn the_default_shutdown_steps_sum_under_a_kubernetes_grace_period() {
    let after = SHUTDOWN_HOOKS_TIMEOUT + FLUSH_TIMEOUT;
    for (transport, stop) in transports() {
        let way_down = stop + after;
        assert!(
            way_down < KUBERNETES_DEFAULT_GRACE,
            "an app serving {transport} takes {way_down:?} to stop by default, which a \
             {KUBERNETES_DEFAULT_GRACE:?} grace period cuts short",
        );
    }
    let longest = transports()
        .into_iter()
        .map(|(_, stop)| stop)
        .max()
        .unwrap_or_default();
    assert_eq!(
        longest + after,
        STATED_WAY_DOWN,
        "the documented way down no longer matches the constants; correct every page that \
         states it",
    );
}
