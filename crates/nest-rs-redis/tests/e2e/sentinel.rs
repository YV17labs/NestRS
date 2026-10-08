//! What holds under Sentinel alone, against the suite's sentinels: the
//! sentinels' own users as the page prescribes them, a service they do not
//! know, a URL declaring the wrong topology — and, one at a time, a real
//! failover under a running queue.

use std::time::Duration;

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError, RedisTopology};

/// A bare client of each sentinel the suite's URL names.
fn sentinels() -> Vec<redis::Client> {
    crate::clients(crate::named_hosts())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sentinels_own_users_are_dialled_as_the_page_prescribes() {
    let user = crate::acl_user("nestrs-e2e-sentinels");
    let rule = crate::documented_acl(crate::CONNECTION_PAGE, "sentinels");
    let _: Vec<()> = crate::on_each(&sentinels(), &crate::creating(&rule, &user)).await;
    let mut url = crate::parsed_url();
    url.query_pairs_mut()
        .append_pair("sentinelUsername", &user)
        .append_pair("sentinelPassword", crate::ACL_PASSWORD);
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: url.to_string(),
        ..crate::redis_config()
    })
    .await
    .expect("the sentinels' user finds the primary");
    let _: () = redis::cmd("PING")
        .query_async(&mut conn)
        .await
        .expect("the primary answers");

    crate::assert_denied_nothing_among(&sentinels(), &user, &[]).await;
    let _: Vec<i64> =
        crate::on_each(&sentinels(), redis::cmd("ACL").arg("DELUSER").arg(&user)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_the_sentinels_do_not_know_fails_the_boot_naming_it() {
    let mut url = crate::parsed_url();
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "sentinelServiceName")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .append_pair("sentinelServiceName", "nestrs-e2e-unknown");
    let Err(refused) = RedisConnection::connect(&RedisConfig {
        url: url.to_string(),
        connect_timeout: Duration::from_millis(500),
        ..crate::redis_config()
    })
    .await
    else {
        panic!("a service no sentinel monitors must not connect");
    };
    assert!(
        matches!(&refused, RedisError::PrimaryUnknown { service_name, .. } if service_name == "nestrs-e2e-unknown"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_url_declaring_a_topology_its_host_is_not_part_of_fails_at_once() {
    let primary = crate::sentinel_primary().await;
    for (url, declared) in [
        (
            format!(
                "rediss-sentinel://{primary}?sentinelServiceName={}",
                crate::service_name()
            ),
            RedisTopology::Sentinel,
        ),
        (
            format!("rediss-cluster://{primary}"),
            RedisTopology::Cluster,
        ),
    ] {
        let started = std::time::Instant::now();
        let Err(refused) = RedisConnection::connect(&RedisConfig {
            url: url.clone(),
            connect_timeout: Duration::from_secs(10),
            ..crate::redis_config()
        })
        .await
        else {
            panic!("{url} must not connect");
        };
        assert!(
            started.elapsed() < crate::harness::AT_ONCE,
            "{url}: at once, took {:?}",
            started.elapsed()
        );
        assert!(
            matches!(&refused, RedisError::TopologyMismatch { declared: found, .. } if *found == declared),
            "{url}: {refused}"
        );
    }
}

/// Failovers move the primary every other test runs on, so they run one at a
/// time, once every other test of the run is done.
mod failover {
    use std::time::Duration;

    use nest_rs_queue::{JobProducerExt, PushOptions, Queue};

    use crate::{FAILOVER, FailoverCommand, FailoverModule, FailoverQueue};

    /// The primary freezes, the sentinels promote its replica, and the app —
    /// its worker and its producer — follows the primary they name then:
    /// every job pushed before, during and after runs, at least once, and a
    /// push after it lands on the new primary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_queue_runs_on_across_a_failover() {
        let logs = nest_rs_testing::LogCapture::install_global();
        let replica = crate::replica_on::<FailoverModule>(crate::redis_config()).await;
        let run = crate::this_run();
        for offset in 0..5 {
            replica
                .producer
                .push(FailoverQueue, FailoverCommand { run: run + offset }, None)
                .await
                .expect("a push before the failover");
        }
        crate::wait_until(Duration::from_secs(10), || {
            (0..5).all(|offset| FAILOVER.finished(run + offset) > 0)
        })
        .await;

        let before = crate::sentinel_primary().await;
        crate::freeze(&before);
        // Forced, the failover skips the sentinels' election, which three
        // sentinels sharing one timing split often enough to stall a test; the
        // primary stays frozen through it, as a crashed one would.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _: () = redis::cmd("SENTINEL")
            .arg("FAILOVER")
            .arg(crate::service_name())
            .query_async(&mut crate::node(&crate::named_hosts()[0]).await)
            .await
            .expect("the sentinels fail the primary over");
        crate::wait_for(Duration::from_secs(30), || async {
            crate::sentinel_primary().await != before
        })
        .await;
        let after = crate::sentinel_primary().await;

        for offset in 5..10 {
            crate::push_through_a_failover(&replica.producer, run + offset).await;
        }
        crate::wait_until(Duration::from_secs(60), || {
            (0..10).all(|offset| FAILOVER.finished(run + offset) > 0)
        })
        .await;
        // A job held back waits in the queue's `due` set: where it is filed is
        // the primary the producer writes to.
        let held = replica
            .producer
            .push(
                FailoverQueue,
                FailoverCommand { run: run + 10 },
                PushOptions::default().with_delay(Duration::from_secs(600)),
            )
            .await
            .expect("a delayed push after the failover");
        let score: Option<f64> = redis::cmd("ZSCORE")
            .arg(crate::key_of(FailoverQueue::NAME, "due"))
            .arg(held.id().to_string())
            .query_async(&mut crate::node(&after).await)
            .await
            .expect("ZSCORE");
        assert!(
            replica.producer.cancel(&held).await.expect("a cancel"),
            "the held job is cancelled"
        );
        replica.worker.shutdown().await.expect("clean shutdown");

        assert!(
            score.is_some(),
            "a push after the failover is filed on the new primary, {after}"
        );
        let followed = logs.find(
            nest_rs_redis::TARGET,
            "redis connection follows the new primary the sentinels name",
        );
        assert!(
            followed
                .iter()
                .any(|event| event.field("primary").as_deref() == Some(after.as_str())),
            "the connection says it follows {after}: {:#?}",
            logs.events(),
        );
    }
}
