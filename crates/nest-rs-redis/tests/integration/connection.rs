//! The shared connection's budget against the nets above it, in process.
//!
//! Every command a Redis binding sends is bounded by the connection's budget,
//! and fails at it with the connection's own sentence — the cause, and the
//! variable to change. A port awaiting a binding waits longer before it gives
//! up on the call, so an outage reaches the caller as that sentence, never as a
//! bare net. Only the order is pinned here; that a binding's call fails within
//! one budget of Redis going silent is proved live, in `e2e`.

use nest_rs_redis::RedisConfig;

/// The default budget sits below the queue port's net and the throttler
/// guard's, with the room each net's documentation argues — twice the budget,
/// for a deployment that raised it — read from the constants that set them
/// rather than retyped.
#[test]
fn the_connection_budget_answers_before_the_queue_port_and_the_throttler_give_up() {
    let budget = RedisConfig::default().connect_timeout;
    for (net, what) in [
        (nest_rs_queue::BACKEND_TIMEOUT, "the queue port's net"),
        (nest_rs_throttler::HIT_TIMEOUT, "the throttler guard's net"),
    ] {
        assert!(
            budget < net,
            "the connection budget ({budget:?}) must answer before {what} ({net:?})"
        );
        assert!(
            budget * 2 <= net,
            "{what} ({net:?}) leaves room for a budget raised to twice its default ({budget:?})"
        );
    }
}
