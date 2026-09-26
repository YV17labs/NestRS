//! `rediss://` against a live Redis. The dev container's Redis speaks plaintext,
//! so each test puts a TLS-terminating proxy in front of it, presenting a
//! certificate the fixtures' test authority signed — `fixtures/README.md` says
//! what each file is.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisError, RedisThrottler, RedisTls, RedisTlsIdentity,
};
use nest_rs_throttler::{Throttle, ThrottlerStore};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

const AUTHORITY: &[u8] = include_bytes!("fixtures/tls_ca.pem");
const SERVER_CERT: &[u8] = include_bytes!("fixtures/tls_server.pem");
const SERVER_KEY: &[u8] = include_bytes!("fixtures/tls_server.key.pem");
const MISNAMED_CERT: &[u8] = include_bytes!("fixtures/tls_misnamed.pem");
const MISNAMED_KEY: &[u8] = include_bytes!("fixtures/tls_misnamed.key.pem");
const CLIENT_CERT: &[u8] = include_bytes!("fixtures/tls_client.pem");
const CLIENT_KEY: &[u8] = include_bytes!("fixtures/tls_client.key.pem");

/// A refusal is one connection attempt, never a budget spent retrying one.
const AT_ONCE: Duration = Duration::from_secs(3);

/// Who the proxy lets finish a handshake.
#[derive(Clone, Copy)]
enum Clients {
    Anyone,
    Certified,
}

/// The server side of a handshake presenting `cert` and `key`, and requiring a
/// certificate the test authority signed when `clients` says so.
fn acceptor(clients: Clients, cert: &[u8], key: &[u8]) -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("the provider speaks the default protocol versions");
    let builder = match clients {
        Clients::Anyone => builder.with_no_client_auth(),
        Clients::Certified => {
            let mut authorities = RootCertStore::empty();
            for cert in CertificateDer::pem_slice_iter(AUTHORITY) {
                authorities
                    .add(cert.expect("the test authority parses"))
                    .expect("and is a trust anchor");
            }
            let verifier =
                WebPkiClientVerifier::builder_with_provider(Arc::new(authorities), provider)
                    .build()
                    .expect("a client verifier over the test authority");
            builder.with_client_cert_verifier(verifier)
        }
    };
    let chain = CertificateDer::pem_slice_iter(cert)
        .collect::<Result<Vec<_>, _>>()
        .expect("the certificate parses");
    let key = PrivateKeyDer::from_pem_slice(key).expect("the key parses");
    TlsAcceptor::from(Arc::new(
        builder
            .with_single_cert(chain, key)
            .expect("the certificate and key correspond"),
    ))
}

/// A TLS-terminating proxy in front of the dev container Redis, on
/// `127.0.0.1` — the address the fixtures' server certificate names.
struct TlsProxy {
    addr: SocketAddr,
    /// Set, every handshake from then on presents the certificate issued for
    /// another host in place of Redis's own — a renewal that installed the
    /// wrong file.
    misnamed: Arc<AtomicBool>,
}

impl TlsProxy {
    async fn start(clients: Clients) -> Self {
        let right = acceptor(clients, SERVER_CERT, SERVER_KEY);
        let wrong = acceptor(clients, MISNAMED_CERT, MISNAMED_KEY);
        let misnamed = Arc::new(AtomicBool::new(false));

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let upstream = crate::redis_address();
        let presents_wrong = Arc::clone(&misnamed);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let acceptor = if presents_wrong.load(Ordering::SeqCst) {
                    wrong.clone()
                } else {
                    right.clone()
                };
                let upstream = upstream.clone();
                tokio::spawn(async move {
                    let mut client = match acceptor.accept(client).into_fallible().await {
                        Ok(client) => client,
                        Err((_, refused)) => return close_gracefully(refused).await,
                    };
                    let Ok(mut server) = TcpStream::connect(&upstream).await else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                });
            }
        });
        Self { addr, misnamed }
    }

    fn present_a_certificate_issued_for_another_host(&self) {
        self.misnamed.store(true, Ordering::SeqCst);
    }

    /// The proxy's URL on database `db`: a test that drops every connection on
    /// its database, or flushes it, takes one of its own.
    fn url_on(&self, db: u8) -> String {
        format!("rediss://{}/{db}", self.addr)
    }
}

/// End a connection whose handshake the proxy refused the way a TLS server
/// does: its alert is already sent, so read what the client sent after it and
/// close. Dropped with that data unread, the socket would answer with a reset,
/// which can reach the client before the alert does and pass for an outage.
async fn close_gracefully(mut refused: TcpStream) {
    let _ = refused.shutdown().await;
    let mut sink = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while refused.read(&mut sink).await.is_ok_and(|read| read > 0) {}
    })
    .await;
}

fn config(url: String, tls: RedisTls) -> RedisConfig {
    RedisConfig {
        url,
        tls,
        ..RedisConfig::default()
    }
}

fn trusting_the_test_authority() -> RedisTls {
    RedisTls {
        ca_cert: Some(AUTHORITY.to_vec()),
        identity: None,
    }
}

/// A certificate the configured authority signed carries the connection: the
/// boot's proof through the handshake, then a caller's own command and the rate
/// limiter's script over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_the_configured_authority_signed_carries_the_connection() {
    let proxy = TlsProxy::start(Clients::Anyone).await;
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

/// A certificate the client does not accept fails the boot at once, sending the
/// operator to the setting that fixes it: the same certificate is refused on
/// every attempt, so retrying it would only spend the budget. Two ways to be
/// refused — signed by an authority nothing trusts, which the authority setting
/// fixes, and trusted but issued for another name, which the URL's host fixes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_the_client_does_not_accept_fails_the_boot_at_once() {
    let untrusted = TlsProxy::start(Clients::Anyone).await;
    let misnamed = TlsProxy::start(Clients::Anyone).await;
    misnamed.present_a_certificate_issued_for_another_host();

    for (case, url, tls, fixed_by, says) in [
        (
            "signed by an authority the client does not trust",
            untrusted.url_on(0),
            RedisTls::default(),
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

/// A Redis that requires a client certificate is handed the configured one, and
/// without one the boot fails at once naming the certificate's settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_requiring_a_client_certificate_is_handed_the_configured_one() {
    let proxy = TlsProxy::start(Clients::Certified).await;
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
    let proxy = TlsProxy::start(Clients::Anyone).await;
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
