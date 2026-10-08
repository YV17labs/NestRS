//! `rediss://` against a live Valkey, which speaks nothing else. Most tests put
//! a proxy in front of it presenting a certificate of the test authority's, so
//! a test can refuse or swap what the client is shown; the queue runs against
//! Valkey itself, every connection verified against the development services'
//! authority.

use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, QueueModule, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule};
use serde::{Deserialize, Serialize};

use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisError, RedisThrottler, RedisTls, RedisTlsIdentity,
};
use nest_rs_testing::TestAuthority;
use nest_rs_throttler::{Throttle, ThrottlerStore};

use crate::Runs;
use crate::harness::AT_ONCE;
use crate::harness::tls::{TlsProxy, config, trusting_the_test_authority};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SealedCommand {
    run: u64,
}

static SEALED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-tls", job = SealedCommand)]
struct SealedQueue;

#[injectable]
#[derive(Default)]
struct SealedProcessor;

#[processor]
impl SealedProcessor {
    #[process(queue = SealedQueue)]
    async fn run(&self, job: SealedCommand) -> anyhow::Result<()> {
        SEALED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
    providers = [SealedProcessor],
)]
struct SealedModule;

/// A queue runs end to end against Valkey speaking TLS itself: the push and
/// the settle on the shared connection, and the worker's blocking read on a
/// connection of its own, each a handshake verified against the configured
/// authority — the server has no plaintext port to fall back to. On a
/// database of its own, the one its blocked read is found on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_runs_over_a_redis_speaking_tls_every_connection_verified() {
    let db = crate::DB_TLS_BLOCKED_READ;
    let mut admin = crate::connect_on(db).await;
    let flush = || redis::cmd("FLUSHDB");
    flush()
        .query_async::<()>(&mut admin)
        .await
        .expect("an empty database to start on");
    let run = crate::this_run();
    let replica = crate::replica_on::<SealedModule>(crate::redis_config_on(db)).await;
    replica
        .producer
        .push(SealedQueue, SealedCommand { run }, None)
        .await
        .expect("a push over TLS");
    crate::wait_until(Duration::from_secs(10), || SEALED.of(run).len() == 1).await;
    let blocked = crate::a_read_blocks_on(db).await;
    replica.worker.shutdown().await.expect("clean shutdown");
    flush()
        .query_async::<()>(&mut admin)
        .await
        .expect("leave the database empty");

    assert_eq!(SEALED.of(run).len(), 1, "the job ran once over TLS");
    assert!(
        blocked,
        "the worker's read blocks on a connection of its own to the TLS Redis",
    );
}

/// The same Valkey under an authority the client was told to trust instead of
/// the system's: the certificate chains to no root it trusts, and the boot
/// fails at once naming what to set — verification is never skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_speaking_tls_under_an_authority_not_trusted_fails_the_boot_at_once() {
    let started = Instant::now();
    let untrusted = RedisConfig {
        tls: trusting_the_test_authority(),
        ..crate::redis_config()
    };
    let Err(error) = RedisConnection::connect(&untrusted).await else {
        panic!("a certificate no trusted authority signed must not connect");
    };
    assert!(started.elapsed() < AT_ONCE, "took {:?}", started.elapsed());
    assert!(matches!(error, RedisError::TlsRefused { .. }), "{error}");
}

/// What holds on one server alone: a double in front of it stands where a
/// Sentinel or Cluster deployment has several hosts.
mod standalone {
    use super::*;

    /// A certificate refused after the boot — a renewal that installed another
    /// host's certificate — is reported at `warn` with what to change, once, as
    /// soon as a command meets the dropped connection. The client reopens the
    /// connection silently and retries for seconds, cutting each caller at its
    /// budget meanwhile, so without the line each caller would read a Redis too
    /// slow to answer, and nothing would name the cause.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_certificate_refused_after_the_boot_is_reported_once_as_the_connection_drops() {
        const REFUSED: &str = "redis refused a reopened tls connection";
        let budget = Duration::from_secs(1);
        let logs = nest_rs_testing::LogCapture::install_global();
        let proxy = TlsProxy::start(Some(crate::redis_url()), None).await;
        let conn = RedisConnection::connect(&RedisConfig {
            connect_timeout: budget,
            ..config(
                proxy.url_on(crate::DB_TLS_REFUSED_REOPEN),
                trusting_the_test_authority(),
            )
        })
        .await
        .expect("connect over TLS");
        redis::cmd("PING")
            .query_async::<()>(&mut conn.clone())
            .await
            .expect("it answers before the renewal");

        proxy.present_a_certificate_issued_for_another_host();
        assert!(
            crate::drop_every_connection_on(crate::DB_TLS_REFUSED_REOPEN).await > 0,
            "the drop reaches the app's connection"
        );

        let met = Instant::now();
        let _ = redis::cmd("PING")
            .query_async::<()>(&mut conn.clone())
            .await;
        crate::wait_until(budget.saturating_sub(met.elapsed()), || {
            !logs.find(nest_rs_redis::TARGET, REFUSED).is_empty()
        })
        .await;
        assert!(
            met.elapsed() < budget,
            "reported before a caller's budget ran out, not once the client stopped retrying: \
             took {:?}",
            met.elapsed()
        );
        for _ in 0..2 {
            assert!(
                redis::cmd("PING")
                    .query_async::<()>(&mut conn.clone())
                    .await
                    .is_err(),
                "a refused certificate carries no command"
            );
        }

        let reported = logs.find(nest_rs_redis::TARGET, REFUSED);
        assert_eq!(
            reported.len(),
            1,
            "reported once, not per caller: {:#?}",
            logs.events()
        );
        let event = &reported[0];
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("endpoint")
                .is_some_and(|endpoint| endpoint.contains(&proxy.addr.to_string())),
            "names the endpoint: {:?}",
            event.fields
        );
        assert!(
            event
                .field("reason")
                .is_some_and(|reason| reason.contains("names no host matching")),
            "and what is wrong with the certificate: {:?}",
            event.fields
        );
    }

    /// A certificate the configured authority signed carries the connection: the
    /// boot's proof through the handshake, then a caller's own command and the rate
    /// limiter's script over it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_certificate_the_configured_authority_signed_carries_the_connection() {
        let proxy = TlsProxy::start(Some(crate::redis_url()), None).await;
        let conn = RedisConnection::connect(&config(
            proxy.url_on(crate::DB_TLS_FLUSH),
            trusting_the_test_authority(),
        ))
        .await
        .expect("connect over TLS");

        redis::cmd("FLUSHDB")
            .query_async::<()>(&mut conn.clone())
            .await
            .expect("a caller's own command answers over TLS");
        let limit = Throttle::new(1, Duration::from_secs(30));
        let throttler = RedisThrottler::new(conn.clone());
        let subject = crate::unique_key("tls");
        assert!(
            throttler.hit(&subject, limit).await.allowed,
            "the rate limiter counts over TLS",
        );
        assert!(
            !throttler.hit(&subject, limit).await.allowed,
            "and its count holds: the second hit is the one over the limit",
        );

        redis::cmd("FLUSHDB")
            .query_async::<()>(&mut conn.clone())
            .await
            .expect("leave the isolated database empty");
    }

    /// A Redis that requires a client certificate is handed the configured one, and
    /// without one the boot fails at once naming the certificate's settings.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_redis_requiring_a_client_certificate_is_handed_the_configured_one() {
        let clients = TestAuthority::new();
        let proxy = TlsProxy::start(Some(crate::redis_url()), Some(&clients)).await;
        let issued = clients.client("nestrs-test-client");
        let presenting = RedisTls {
            identity: Some(RedisTlsIdentity {
                cert: issued.cert.into_bytes(),
                key: issued.key.into_bytes(),
            }),
            ..trusting_the_test_authority()
        };
        let conn = RedisConnection::connect(&config(proxy.url_on(0), presenting))
            .await
            .expect("connect presenting the client certificate");
        redis::cmd("PING")
            .query_async::<()>(&mut conn.clone())
            .await
            .expect("and it answers");

        let started = Instant::now();
        let Err(error) =
            RedisConnection::connect(&config(proxy.url_on(0), trusting_the_test_authority())).await
        else {
            panic!("a Redis requiring a client certificate must refuse a client presenting none")
        };
        let took = started.elapsed();
        assert!(matches!(error, RedisError::TlsRefused { .. }), "{error}");
        assert!(
            took < AT_ONCE,
            "a refused client spends none of the budget, took {took:?}"
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis", "TLS_CERT")),
            "the error names the client certificate's setting: {error}",
        );
        assert!(
            rendered.contains("requires a client certificate"),
            "and says that is what Redis refused: {error}",
        );
    }
}
