//! The TLS doubles both suites dial: a proxy terminating `rediss://` with a
//! certificate the fixtures' test authority signed — `fixtures/README.md` says
//! what each file is.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_redis::{RedisConfig, RedisTls};
use nest_rs_testing::{url_at, url_on};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use super::address_of;

pub(crate) const AUTHORITY: &[u8] = include_bytes!("fixtures/tls_ca.pem");
const SERVER_CERT: &[u8] = include_bytes!("fixtures/tls_server.pem");
const SERVER_KEY: &[u8] = include_bytes!("fixtures/tls_server.key.pem");
const MISNAMED_CERT: &[u8] = include_bytes!("fixtures/tls_misnamed.pem");
const MISNAMED_KEY: &[u8] = include_bytes!("fixtures/tls_misnamed.key.pem");

/// The server side of a handshake presenting `cert` and `key`, and requiring a
/// client certificate `clients_signed_by` signed when it names an authority.
fn acceptor(clients_signed_by: Option<&[u8]>, cert: &[u8], key: &[u8]) -> TlsAcceptor {
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

/// A TLS-terminating proxy on `127.0.0.1` — the address the fixtures' server
/// certificate names — forwarding each accepted handshake to the plaintext
/// Redis `upstream` names, or closing it when there is none: an in-process
/// test names no Redis.
pub(crate) struct TlsProxy {
    pub(crate) addr: SocketAddr,
    /// The URL of the Redis it fronts, as a client reaches it: over TLS.
    fronts: String,
    /// Set, every handshake from then on presents the certificate issued for
    /// another host in place of Redis's own — a renewal that installed the
    /// wrong file.
    misnamed: Arc<AtomicBool>,
}

impl TlsProxy {
    pub(crate) async fn start(
        upstream: Option<String>,
        clients_signed_by: Option<&'static [u8]>,
    ) -> Self {
        let right = acceptor(clients_signed_by, SERVER_CERT, SERVER_KEY);
        let wrong = acceptor(clients_signed_by, MISNAMED_CERT, MISNAMED_KEY);
        let misnamed = Arc::new(AtomicBool::new(false));
        let dialled = upstream.as_deref().map(address_of);
        let fronts = match &upstream {
            Some(url) => format!(
                "rediss://{}",
                url.strip_prefix("redis://")
                    .expect("the proxy forwards to a plaintext Redis")
            ),
            None => "rediss://localhost".to_owned(),
        };

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
                let dialled = dialled.clone();
                tokio::spawn(async move {
                    let mut client = match acceptor.accept(client).into_fallible().await {
                        Ok(client) => client,
                        Err((_, refused)) => return close_gracefully(refused).await,
                    };
                    let Some(dialled) = dialled else {
                        return;
                    };
                    let Ok(mut server) = TcpStream::connect(&dialled).await else {
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

pub(crate) fn trusting_the_test_authority() -> RedisTls {
    RedisTls {
        ca_cert: Some(AUTHORITY.to_vec()),
        identity: None,
    }
}
