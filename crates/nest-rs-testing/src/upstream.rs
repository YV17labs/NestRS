//! [`Upstream`] — the development service a TLS double stands in front of, and
//! the two handshakes the double runs for it: the client's, accepted with a
//! [`TestCertificate`](crate::TestCertificate)'s acceptor, and the service's,
//! dialled and verified against the system's authorities
//! ([`system_connector`]).

use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, client, server};

use crate::system_connector;

/// The `SSLRequest` a Postgres client opens with before its handshake, and the
/// server answers `S` to (the protocol's *SSL Session Encryption*).
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

/// A development service a TLS double fronts: its host — the name its
/// certificate carries — and port, and what its protocol sends before the
/// handshake. The double ends TLS on both sides, so what it reads or holds back
/// is the protocol, never a record.
///
/// ```no_run
/// use nest_rs_testing::{TestAuthority, Upstream};
///
/// # async fn relay() {
/// let authority = TestAuthority::new();
/// let acceptor = authority.server(&["127.0.0.1"]).acceptor(None);
/// let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
/// let upstream = Upstream::postgres("postgres", 5432);
/// let (client, _) = listener.accept().await.unwrap();
/// if let Some((mut client, mut server)) = upstream.bridge(client, &acceptor).await {
///     let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
/// }
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Upstream {
    host: String,
    port: u16,
    opening: Opening,
}

/// What a protocol sends before its TLS handshake.
#[derive(Clone, Copy, Debug)]
enum Opening {
    /// Nothing: TLS from the first byte (Valkey, S3).
    None,
    /// Postgres's `SSLRequest`, answered `S`.
    Postgres,
}

impl Upstream {
    /// A service speaking TLS from its first byte — Valkey, an S3 store — at
    /// `host` and `port`.
    pub fn tls(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            opening: Opening::None,
        }
    }

    /// A Postgres server at `host` and `port`, whose client asks for TLS with an
    /// `SSLRequest` first.
    pub fn postgres(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            opening: Opening::Postgres,
        }
    }

    /// `client`'s handshake, taken as the service takes it — its protocol's
    /// opening answered, then TLS accepted through `acceptor`; `None` when the
    /// client gave up on it.
    pub async fn accept(
        &self,
        mut client: TcpStream,
        acceptor: &TlsAcceptor,
    ) -> Option<server::TlsStream<TcpStream>> {
        if let Opening::Postgres = self.opening {
            let mut request = [0_u8; 8];
            client.read_exact(&mut request).await.ok()?;
            if request != SSL_REQUEST {
                return None;
            }
            client.write_all(b"S").await.ok()?;
        }
        acceptor.accept(client).await.ok()
    }

    /// The service reached over TLS, its certificate verified against the
    /// system's authorities, after its protocol's opening; `None` when it
    /// cannot be reached or refuses.
    pub async fn dial(&self) -> Option<client::TlsStream<TcpStream>> {
        let mut server = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .ok()?;
        if let Opening::Postgres = self.opening {
            server.write_all(&SSL_REQUEST).await.ok()?;
            let mut encrypts = [0_u8; 1];
            server.read_exact(&mut encrypts).await.ok()?;
            if encrypts != *b"S" {
                return None;
            }
        }
        let name = ServerName::try_from(self.host.clone()).ok()?;
        system_connector().connect(name, server).await.ok()
    }

    /// Both ends of a double between `client` and the service: the client's
    /// handshake [accepted](Self::accept), the service [dialled](Self::dial) —
    /// `None` when either side gave up.
    pub async fn bridge(
        &self,
        client: TcpStream,
        acceptor: &TlsAcceptor,
    ) -> Option<(server::TlsStream<TcpStream>, client::TlsStream<TcpStream>)> {
        let client = self.accept(client, acceptor).await?;
        let server = self.dial().await?;
        Some((client, server))
    }
}
