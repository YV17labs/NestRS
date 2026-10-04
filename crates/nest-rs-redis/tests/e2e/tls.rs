//! `rediss://` against a live Redis. The dev container's Redis speaks plaintext,
//! so each test puts a TLS-terminating proxy in front of it, presenting a
//! certificate the fixtures' test authority signed — `fixtures/README.md` says
//! what each file is.

use std::time::{Duration, Instant};

use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisError, RedisThrottler, RedisTls, RedisTlsIdentity,
};
use nest_rs_throttler::{Throttle, ThrottlerStore};

use crate::harness::AT_ONCE;
use crate::harness::tls::{AUTHORITY, TlsProxy, config, trusting_the_test_authority};

const CLIENT_CERT: &[u8] = include_bytes!("../harness/fixtures/tls_client.pem");
const CLIENT_KEY: &[u8] = include_bytes!("../harness/fixtures/tls_client.key.pem");

/// A certificate the configured authority signed carries the connection: the
/// boot's proof through the handshake, then a caller's own command and the rate
/// limiter's script over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_the_configured_authority_signed_carries_the_connection() {
    let proxy = TlsProxy::start(Some(crate::redis_address()), None).await;
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
    let proxy = TlsProxy::start(Some(crate::redis_address()), Some(AUTHORITY)).await;
    let presenting = RedisTls {
        identity: Some(RedisTlsIdentity {
            cert: CLIENT_CERT.to_vec(),
            key: CLIENT_KEY.to_vec(),
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

/// A certificate refused after the boot — a renewal that installed another
/// host's certificate — is reported at `warn` with what to change, once, when
/// the connection reopens against it. The client reopens the connection
/// silently, so without the line each caller's command would just fail, and
/// nothing would name the cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_refused_after_the_boot_is_reported_once_when_the_connection_reopens() {
    const REFUSED: &str = "redis refused a reopened tls connection";
    let logs = nest_rs_testing::LogCapture::install_global();
    let proxy = TlsProxy::start(Some(crate::redis_address()), None).await;
    let conn = RedisConnection::connect(&RedisConfig {
        connect_timeout: Duration::from_secs(2),
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

    let deadline = Instant::now() + Duration::from_secs(45);
    while logs.find(nest_rs_redis::TARGET, REFUSED).is_empty() && Instant::now() < deadline {
        let _ = redis::cmd("PING")
            .query_async::<()>(&mut conn.clone())
            .await;
    }
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
