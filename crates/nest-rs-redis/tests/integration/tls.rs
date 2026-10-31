//! `rediss://` without a Redis: a certificate the client does not accept is
//! refused at the handshake, before the proxy would forward a byte.

use nest_rs_config::ClientTls;
use nest_rs_redis::{RedisConnection, RedisError};
use nest_rs_testing::LogCapture;

use crate::harness::no_attempt_retried;
use crate::harness::tls::{TlsProxy, config, trusting_the_test_authority};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_the_client_does_not_accept_fails_the_boot_at_once() {
    let logs = LogCapture::install();
    let untrusted = TlsProxy::start(None, None).await;
    let misnamed = TlsProxy::start(None, None).await;
    misnamed.present_a_certificate_issued_for_another_host();

    for (case, proxy, tls, fixed_by, says) in [
        (
            "signed by an authority the client does not trust",
            &untrusted,
            ClientTls::default(),
            "TLS_CA_CERT",
            "does not chain to an authority",
        ),
        (
            "issued for a name the URL does not dial",
            &misnamed,
            trusting_the_test_authority(),
            "URL",
            "subjectAltName",
        ),
    ] {
        let Err(error) = RedisConnection::connect(&config(proxy.url_on(0), tls)).await else {
            panic!("a certificate {case} must not connect")
        };
        assert!(
            matches!(error, RedisError::TlsRefused { .. }),
            "{case}: {error}"
        );
        no_attempt_retried(&logs, case);
        assert_eq!(proxy.handshakes(), 1, "{case}: one attempt, refused");
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
