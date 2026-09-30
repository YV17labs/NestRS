//! The way down, read across the crates that bound it: the HTTP transport's
//! window, the kernel's shutdown-hooks budget, the telemetry flush. No crate
//! owns the sum, so it is asserted here, where all three are reachable.

/// The three bounded steps on the way down — the HTTP window, the shutdown
/// hooks' budget, the telemetry flush — sum under the grace a Kubernetes pod is
/// given by default between `SIGTERM` and `SIGKILL`, with room left over. Past
/// it the kill lands first and says nothing, taking the hooks and the flush with
/// it. Read off the three constants, so moving any one of them past the sum
/// fails here rather than in a rollout.
#[cfg(feature = "opentelemetry")]
#[test]
fn the_default_shutdown_steps_sum_under_a_kubernetes_grace_period() {
    /// `terminationGracePeriodSeconds`' default: Kubernetes' number, not ours.
    const KUBERNETES_DEFAULT_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

    // The settle wait is the transport's too: `serve` spends it after the
    // window, stopping what its self-mounts ran off their connections.
    let steps = nest_rs_http::HttpConfig::default().shutdown_timeout
        + nest_rs_http::DetachedWork::SETTLE_TIMEOUT
        + nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT
        + nest_rs_opentelemetry::FLUSH_TIMEOUT;
    assert!(
        steps < KUBERNETES_DEFAULT_GRACE,
        "the default shutdown takes {steps:?}, which a {KUBERNETES_DEFAULT_GRACE:?} grace \
         period cuts short",
    );
}
