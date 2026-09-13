//! Opening the shared pool against a live Redis — the one branch of
//! `RedisConnection::connect_within` a local listener cannot reach, because it
//! needs a server that answers and refuses.

use std::time::{Duration, Instant};

use nest_rs_redis::RedisConnection;

/// Credentials Redis refuses are not an outage. Retried as one, the boot spent
/// its whole budget and then told the operator to check the network and widen
/// the timeout.
#[tokio::test]
async fn credentials_redis_refuses_fail_the_boot_at_once_naming_them() {
    let url = crate::redis_url().replacen("://", "://:nestrs-e2e-wrong-secret@", 1);
    let started = Instant::now();
    let Err(error) = RedisConnection::connect_within(&url, Duration::from_secs(10)).await else {
        panic!("a password Redis does not accept must not connect")
    };

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "a refusal spends none of the budget, took {:?}",
        started.elapsed(),
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("refused the connection"),
        "the error says what Redis answered: {rendered}",
    );
    assert!(
        !rendered.contains("nestrs-e2e-wrong-secret"),
        "and never shows them: {rendered}",
    );
}
