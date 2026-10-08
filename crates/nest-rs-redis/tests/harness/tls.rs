//! TLS for both suites, which reach Valkey over nothing else: the test
//! authority every double's certificate is issued by — in process, none is
//! committed — the handshakes a double accepts and dials, and a proxy that
//! can present a certificate issued for another host.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use nest_rs_redis::{RedisConfig, RedisTls};
use nest_rs_testing::{TestAuthority, TestCertificate, url_at, url_on};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector, client, server};

/// The authority this test process issues its doubles' certificates with.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

/// What a double presents: the loopback it listens on.
static DOUBLE: LazyLock<TestCertificate> =
    LazyLock::new(|| AUTHORITY.server(&["127.0.0.1", "localhost"]));

/// What a double presents once told to: a certificate for another host.
static MISNAMED: LazyLock<TestCertificate> =
    LazyLock::new(|| AUTHORITY.server(&["elsewhere.nestrs.test"]));

/// The handshake a double accepts, with no client certificate asked.
static ACCEPTOR: LazyLock<TlsAcceptor> = LazyLock::new(|| acceptor(None, &DOUBLE));

/// The test authority's certificate, which a client trusts to reach a double.
pub(crate) fn authority() -> &'static [u8] {
    AUTHORITY.pem().as_bytes()
}

/// What every connection of the suites trusts: the system's authorities —
/// where the dev container and CI install the development services' — to
/// reach Valkey, and the test authority, to reach a double.
pub(crate) fn trusted() -> RedisTls {
    let mut authorities = authority().to_vec();
    authorities.push(b'\n');
    authorities.extend_from_slice(nest_rs_config::system_authorities());
    RedisTls {
        ca_cert: Some(authorities),
        identity: None,
    }
}

/// The test authority alone, so the development services' certificate is
/// one the client does not trust.
pub(crate) fn trusting_the_test_authority() -> RedisTls {
    RedisTls {
        ca_cert: Some(authority().to_vec()),
        identity: None,
    }
}

/// Accept `client`'s handshake as a double, presenting the loopback's
/// certificate; `None` when the client gave up on it.
pub(crate) async fn accept(client: TcpStream) -> Option<server::TlsStream<TcpStream>> {
    ACCEPTOR.accept(client).await.ok()
}

/// Reach the Valkey `url` names over TLS, verifying its certificate against
/// the system's authorities; `None` when it cannot be reached.
pub(crate) async fn dial(url: &str) -> Option<client::TlsStream<TcpStream>> {
    let info = redis::IntoConnectionInfo::into_connection_info(url)
        .expect("the URL of the Valkey a double fronts parses");
    let host = match info.addr() {
        redis::ConnectionAddr::TcpTls { host, .. } | redis::ConnectionAddr::Tcp(host, _) => {
            host.clone()
        }
        other => panic!("a double fronts Valkey over TCP, not {other}"),
    };
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(nest_rs_config::system_authorities()) {
        let _ = roots.add(cert.expect("the system's store parses"));
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the provider speaks the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(host).expect("the host is a server name");
    let tcp = TcpStream::connect(info.addr().to_string()).await.ok()?;
    TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .ok()
}

/// The server side of a handshake presenting `issued`, and requiring a client
/// certificate `clients_signed_by` signed when it names an authority.
fn acceptor(clients_signed_by: Option<&[u8]>, issued: &TestCertificate) -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("the provider speaks the default protocol versions");
    let builder = match clients_signed_by {
        None => builder.with_no_client_auth(),
        Some(authority) => {
            let mut authorities = RootCertStore::empty();
            for cert in CertificateDer::pem_slice_iter(authority) {
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
    let chain = CertificateDer::pem_slice_iter(issued.cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("the certificate parses");
    let key = PrivateKeyDer::from_pem_slice(issued.key.as_bytes()).expect("the key parses");
    TlsAcceptor::from(Arc::new(
        builder
            .with_single_cert(chain, key)
            .expect("the certificate and key correspond"),
    ))
}

/// A proxy on `127.0.0.1` terminating TLS with the test authority's
/// certificate — or one issued for another host, once told to — and
/// forwarding each accepted handshake to the Valkey `upstream` names, over TLS,
/// or closing it when there is none: an in-process test names no Valkey.
pub(crate) struct TlsProxy {
    pub(crate) addr: SocketAddr,
    /// The URL of the Valkey it fronts, as a client reaches it.
    fronts: String,
    /// Set, every handshake from then on presents the certificate issued for
    /// another host in place of the right one — a renewal that installed the
    /// wrong file.
    misnamed: Arc<AtomicBool>,
}

impl TlsProxy {
    /// `clients_signed_by` asks every client for a certificate that authority
    /// issued, as a server with `tls-auth-clients yes` does.
    pub(crate) async fn start(upstream: Option<String>, clients_signed_by: Option<&[u8]>) -> Self {
        let right = acceptor(clients_signed_by, &DOUBLE);
        let wrong = acceptor(clients_signed_by, &MISNAMED);
        let misnamed = Arc::new(AtomicBool::new(false));
        let fronts = upstream
            .clone()
            .unwrap_or_else(|| "rediss://localhost".to_owned());

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
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
                    let Some(upstream) = upstream else {
                        return;
                    };
                    let Some(mut server) = dial(&upstream).await else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                });
            }
        });
        Self {
            addr,
            fronts,
            misnamed,
        }
    }

    pub(crate) fn present_a_certificate_issued_for_another_host(&self) {
        self.misnamed.store(true, Ordering::SeqCst);
    }

    /// The proxy's URL on database `db`: a test that drops every connection on
    /// its database, or flushes it, takes one of its own.
    pub(crate) fn url_on(&self, db: u8) -> String {
        url_at(&url_on(&self.fronts, db), self.addr)
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

pub(crate) fn config(url: String, tls: RedisTls) -> RedisConfig {
    RedisConfig {
        url,
        tls,
        ..RedisConfig::default()
    }
}
