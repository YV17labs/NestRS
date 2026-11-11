//! `nest-rs-seaorm`'s in-process suite: what the exposure decorators emit, in
//! `expose/`, which mirrors their macro crate's `src/` — their refusals are
//! trybuild snapshots in `nest-rs-macro-hygiene`'s suite — the pool's refusal
//! of a server it cannot verify, before any login, the indicator the pool's
//! module attaches, in `health/`, the unit of work under the
//! HTTP edge's deadline, a page cursor refused at every edge reading one, and
//! what a `#[resource]`'s binding answers over a service standing in for the row.
//! A server's refusal reaches `Repo` from a Postgres double, so what it hands
//! out is held to what it prints with no server running.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

#[path = "../harness/always_authenticated.rs"]
mod always_authenticated;
mod bind;
mod database;
mod error;
mod expose;
mod graphql;
mod health;
mod http;
mod module;
mod page;
mod repo;
mod savepoint;
mod service;
#[path = "../harness/system_store.rs"]
mod system_store;
mod unit;

/// The principal the suite's authentication stand-ins attach: a caller with no
/// identity of its own.
#[derive(Clone, Debug, Default)]
pub struct Anyone;

impl nest_rs_core::AmbientValue for Anyone {}

/// A test app over a pool and a config seeded unconnected, for a module that
/// imports `SeaOrmDatabaseModule` and refuses before any statement runs.
pub(crate) fn over_an_unconnected_pool() -> nest_rs_testing::TestAppBuilder {
    nest_rs_testing::TestApp::builder()
        .provide(sea_orm::DatabaseConnection::default())
        .provide(nest_rs_seaorm::SeaOrmConfig::default())
}

/// A test app over a pool whose every acquire is refused: nothing listens on port 1.
pub(crate) async fn over_a_pool_refusing_every_acquire() -> nest_rs_testing::TestAppBuilder {
    let mut options = sea_orm::ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
    options
        .connect_lazy(true)
        .acquire_timeout(std::time::Duration::from_millis(50));
    let db = sea_orm::Database::connect(options)
        .await
        .expect("a lazy pool opens nothing");
    nest_rs_testing::TestApp::builder().provide(db)
}

/// The `SSLRequest` a Postgres client opens with: its length, then the code
/// 80877103.
pub(crate) const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

/// One Postgres message: its tag, then its length, which counts itself.
pub(crate) fn postgres_message(tag: u8, body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len() + 4).expect("a short message");
    let mut message = vec![tag];
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(body);
    message
}

/// A Postgres double over TLS, its certificate issued by a test authority, letting
/// every client in and answering each statement with the `ErrorResponse` `fields`
/// make (each a field's kind and its value), then `ReadyForQuery`; a bare `Sync`, a
/// pool's ping, is answered ready. It answers at the statement's `Parse`, where a
/// server meets a constraint at its `Execute`: sqlx decodes either alike. The URL
/// it hands back trusts the authority through the file handed back beside it, which
/// the test holds while it connects.
pub(crate) async fn postgres_refusing_every_statement(
    fields: &[(u8, &str)],
) -> (String, tempfile::NamedTempFile) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let authority = nest_rs_testing::TestAuthority::new();
    let root = tempfile::NamedTempFile::new().expect("a file for the authority");
    std::fs::write(root.path(), authority.pem()).expect("the authority's certificate");
    let mut body = Vec::new();
    for (kind, value) in fields {
        body.push(*kind);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut refusal = postgres_message(b'E', &body);
    let ready = postgres_message(b'Z', b"I");
    refusal.extend_from_slice(&ready);
    let mut welcome = postgres_message(b'R', &0_u32.to_be_bytes());
    welcome.extend_from_slice(&ready);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let port = listener.local_addr().expect("its address").port();
    let acceptor = authority.server(&["127.0.0.1", "localhost"]).acceptor(None);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (acceptor, refusal, ready, welcome) = (
                acceptor.clone(),
                refusal.clone(),
                ready.clone(),
                welcome.clone(),
            );
            tokio::spawn(async move {
                let mut request = [0_u8; 8];
                if socket.read_exact(&mut request).await.is_err()
                    || request != SSL_REQUEST
                    || socket.write_all(b"S").await.is_err()
                {
                    return;
                }
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    return;
                };
                let mut length = [0_u8; 4];
                if tls.read_exact(&mut length).await.is_err() {
                    return;
                }
                let rest = usize::try_from(u32::from_be_bytes(length))
                    .unwrap_or(0)
                    .saturating_sub(4);
                if tls.read_exact(&mut vec![0_u8; rest]).await.is_err()
                    || tls.write_all(&welcome).await.is_err()
                {
                    return;
                }
                let mut statement = false;
                loop {
                    let mut header = [0_u8; 5];
                    if tls.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let [tag, length @ ..] = header;
                    let rest = usize::try_from(u32::from_be_bytes(length))
                        .unwrap_or(0)
                        .saturating_sub(4);
                    if tls.read_exact(&mut vec![0_u8; rest]).await.is_err() {
                        return;
                    }
                    let answer = match tag {
                        b'X' => return,
                        b'Q' => &refusal,
                        b'S' if std::mem::take(&mut statement) => &refusal,
                        b'S' => &ready,
                        _ => {
                            statement = true;
                            continue;
                        }
                    };
                    if tls.write_all(answer).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    let url = format!(
        "postgres://nestrs@127.0.0.1:{port}/postgres?sslmode=verify-full&sslrootcert={}",
        root.path().display()
    );
    (url, root)
}
