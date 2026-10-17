//! `rediss://` without a Redis: a certificate the client does not accept is
//! refused at the handshake, before the proxy would forward a byte.

use std::time::Instant;

use nest_rs_config::ClientTls;
use nest_rs_redis::{RedisConnection, RedisError};

use crate::harness::AT_ONCE;
use crate::harness::tls::{TlsProxy, config, trusting_the_test_authority};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_the_client_does_not_accept_fails_the_boot_at_once() {
    let untrusted = TlsProxy::start(None, None).await;
    let misnamed = TlsProxy::start(None, None).await;
    misnamed.present_a_certificate_issued_for_another_host();

    for (case, url, tls, fixed_by, says) in [
        (
            "signed by an authority the client does not trust",
            untrusted.url_on(0),
            ClientTls::default(),
            "TLS_CA_CERT",
            "does not chain to an authority",
        ),
        (
            "issued for a name the URL does not dial",
            misnamed.url_on(0),
            trusting_the_test_authority(),
            "URL",
            "subjectAltName",
        ),
    ] {
        let started = Instant::now();
        let Err(error) = RedisConnection::connect(&config(url, tls)).await else {
            panic!("a certificate {case} must not connect")
        };
        let took = started.elapsed();
        assert!(
            matches!(error, RedisError::TlsRefused { .. }),
            "{case}: {error}"
        );
        assert!(
            took < AT_ONCE,
            "{case}: a refused certificate spends none of the budget, took {took:?}",
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis", fixed_by)),
            "{case}: the error names the setting that fixes it: {error}",
        );
        assert!(
            rendered.contains(says),
            "{case}: and gives the cause that setting fixes, not another's: {error}",
        );
    }
}
