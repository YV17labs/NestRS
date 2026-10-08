//! What holds under Sentinel alone, against the suite's sentinels: the
//! sentinels' own users as the page prescribes them, a service they do not
//! know, a URL declaring the wrong topology — and, one at a time, a real
//! failover under a running queue.

use std::time::Duration;

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError, RedisTopology};

/// The name the suite's sentinels monitor the primary under.
fn service_name() -> String {
    crate::parsed_url()
        .query_pairs()
        .find(|(key, _)| key == "sentinelServiceName")
        .map(|(_, name)| name.into_owned())
        .expect("a Sentinel URL names its primary")
}

/// A connection to each sentinel the suite's URL names.
async fn sentinels() -> Vec<redis::aio::MultiplexedConnection> {
    let mut sentinels = Vec::new();
    for host in crate::named_hosts() {
        sentinels.push(
            crate::bare_client(&crate::node_url(&host))
                .get_multiplexed_async_connection()
                .await
                .expect("every sentinel answers"),
        );
    }
    sentinels
}

/// The primary the sentinels name now, as `host:port`.
async fn primary() -> String {
    let (host, port): (String, u16) = redis::cmd("SENTINEL")
        .arg("GET-MASTER-ADDR-BY-NAME")
        .arg(service_name())
        .query_async(&mut sentinels().await.remove(0))
        .await
        .expect("the sentinels name a primary");
    format!("{host}:{port}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sentinels_own_users_are_dialled_as_the_page_prescribes() {
    let user = crate::acl_user("nestrs-e2e-sentinels");
    let rule = crate::documented_acl(crate::CONNECTION_PAGE, "sentinels")
        .replace("<user>", &user)
        .replace("<password>", crate::ACL_PASSWORD);
    let tokens: Vec<&str> = rule.split_whitespace().collect();
    for mut sentinel in sentinels().await {
        let _: () = redis::cmd(tokens[0])
            .arg(&tokens[1..])
            .query_async(&mut sentinel)
            .await
            .unwrap_or_else(|error| panic!("a sentinel takes the page's rule `{rule}`: {error}"));
    }
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

    for mut sentinel in sentinels().await {
        let denied: Vec<std::collections::HashMap<String, redis::Value>> = redis::cmd("ACL")
            .arg("LOG")
            .arg(i64::from(u32::MAX))
            .query_async(&mut sentinel)
            .await
            .expect("ACL LOG");
        let _: i64 = redis::cmd("ACL")
            .arg("DELUSER")
            .arg(&user)
            .query_async(&mut sentinel)
            .await
            .expect("ACL DELUSER");
        let by_user = denied
            .iter()
            .filter(|entry| {
                entry
                    .get("username")
                    .and_then(|name| redis::from_redis_value_ref::<String>(name).ok())
                    .is_some_and(|name| name == user)
            })
            .count();
        assert_eq!(by_user, 0, "a sentinel denied its user a command");
    }
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
    let primary = primary().await;
    for (url, declared) in [
        (
            format!(
                "rediss-sentinel://{primary}?sentinelServiceName={}",
                service_name()
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

    use nest_rs_core::{injectable, module};
    use nest_rs_queue::{JobProducerExt, QueueModule, processor, queue};
    use nest_rs_redis::{RedisModule, RedisQueueModule};
    use serde::{Deserialize, Serialize};

    use crate::Runs;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct FailoverCommand {
        run: u64,
    }

    static FAILOVER: Runs = Runs::new();

    #[queue(name = "nestrs-e2e-sentinel-failover", job = FailoverCommand)]
    struct FailoverQueue;

    #[injectable]
    #[derive(Default)]
    struct FailoverProcessor;

    #[processor]
    impl FailoverProcessor {
        #[process(queue = FailoverQueue, retries = 5)]
        async fn run(&self, job: FailoverCommand) -> anyhow::Result<()> {
            FAILOVER.start(job.run);
            FAILOVER.finish(job.run);
            Ok(())
        }
    }

    #[module(
        imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
        providers = [FailoverProcessor],
    )]
    struct FailoverModule;

    /// The primary freezes, the sentinels promote its replica, and the app —
    /// its worker and its producer — follows the primary they name then:
    /// every job pushed before, during and after runs, at least once.
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

        let before = super::primary().await;
        let frozen = crate::bare_client(&crate::node_url(&before));
        tokio::spawn(async move {
            if let Ok(mut frozen) = frozen.get_multiplexed_async_connection().await {
                let _: Result<(), _> = redis::cmd("DEBUG")
                    .arg("SLEEP")
                    .arg(6)
                    .query_async(&mut frozen)
                    .await;
            }
        });
        // Forced, the failover skips the sentinels' election, which three
        // sentinels sharing one timing split often enough to stall a test; the
        // primary stays frozen through it, as a crashed one would.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _: () = redis::cmd("SENTINEL")
            .arg("FAILOVER")
            .arg(super::service_name())
            .query_async(&mut super::sentinels().await.remove(0))
            .await
            .expect("the sentinels fail the primary over");
        let mut after = before.clone();
        for _ in 0..300 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            after = super::primary().await;
            if after != before {
                break;
            }
        }
        assert_ne!(after, before, "the sentinels promote the replica");

        for offset in 5..10 {
            let job = FailoverCommand { run: run + offset };
            let mut pushed = false;
            for _ in 0..100 {
                if replica
                    .producer
                    .push(FailoverQueue, job.clone(), None)
                    .await
                    .is_ok()
                {
                    pushed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            assert!(pushed, "a push answers again once the primary moved");
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
                nest_rs_queue::PushOptions::default().with_delay(Duration::from_secs(600)),
            )
            .await
            .expect("a delayed push after the failover");
        let score: Option<f64> = redis::cmd("ZSCORE")
            .arg(crate::key_of(
                <FailoverQueue as nest_rs_queue::Queue>::NAME,
                "due",
            ))
            .arg(held.id().to_string())
            .query_async(
                &mut crate::bare_client(&crate::node_url(&after))
                    .get_multiplexed_async_connection()
                    .await
                    .expect("the new primary answers"),
            )
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

        for offset in 0..10 {
            assert!(
                FAILOVER.finished(run + offset) > 0,
                "job {offset} ran at least once"
            );
        }
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
