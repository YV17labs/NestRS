//! Covers `src/database.rs`: the fixture opens its connections as the app's
//! pool does — TLS verified, never encrypted without a check nor plaintext
//! because a server declined it — and what a server refuses it, a migration
//! included, is said by its SQLSTATE, never the value the server quoted.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_testing::{EphemeralDatabase, TestAuthority};
use sea_orm_migration::{MigrationTrait, MigratorTrait};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct NoMigrations;

impl MigratorTrait for NoMigrations {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Vec::new()
    }
}

/// The `SSLRequest` a Postgres client opens with: its length, then the code
/// 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

/// How a Postgres double answers a client's `SSLRequest`.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// `S`, then a handshake presenting a certificate of an authority the
    /// system does not trust.
    Untrusted,
    /// `N`: the server has no TLS.
    Declined,
}

/// A Postgres double answering every `SSLRequest` as `answer` says, and
/// recording whether a client went on to send its startup message — what it
/// sends only once it accepted the connection, credentials included.
async fn double(answer: Answer) -> (String, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local listener");
    let port = listener.local_addr().expect("a bound address").port();
    let started = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&started);
    let acceptor = TestAuthority::new()
        .server(&["127.0.0.1", "localhost"])
        .acceptor(None);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 8];
            if socket.read_exact(&mut request).await.is_err() || request != SSL_REQUEST {
                continue;
            }
            let mut startup = [0u8; 8];
            let read = match answer {
                Answer::Untrusted => {
                    if socket.write_all(b"S").await.is_err() {
                        continue;
                    }
                    match acceptor.accept(socket).await {
                        Ok(mut tls) => tls.read_exact(&mut startup).await.is_ok(),
                        Err(_) => false,
                    }
                }
                Answer::Declined => {
                    socket.write_all(b"N").await.is_ok()
                        && socket.read_exact(&mut startup).await.is_ok()
                }
            };
            if read {
                seen.store(true, Ordering::SeqCst);
            }
        }
    });
    (
        format!("postgres://nestrs:s3cret@127.0.0.1:{port}/postgres"),
        started,
    )
}

#[tokio::test]
async fn the_fixture_never_logs_in_to_a_server_it_cannot_verify() {
    for answer in [Answer::Untrusted, Answer::Declined] {
        let (url, started) = double(answer).await;
        let Err(refused) = EphemeralDatabase::create_with::<NoMigrations>(&url).await else {
            panic!("{answer:?}: the fixture opened a database on a server it could not verify");
        };
        assert!(
            !started.load(Ordering::SeqCst),
            "{answer:?}: the fixture sent its startup message, credentials included: {refused:#}"
        );
        assert!(!format!("{refused:#}").contains("s3cret"), "{refused:#}");
    }
}

/// The value the double's refusal quotes, as Postgres quotes the input it refused.
const MARKED: &str = "zzqx-ada@example.com";

/// One Postgres message: its tag, then its length, which counts itself.
fn postgres_message(tag: u8, body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len() + 4).expect("a short message");
    let mut message = vec![tag];
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(body);
    message
}

/// A Postgres double over TLS, its certificate issued by `authority`, letting
/// every client in and refusing each statement with a `22P02` whose message
/// quotes [`MARKED`], as Postgres quotes a value its column's type refuses; a
/// bare `Sync`, a pool's ping, is answered ready. The URL trusts the authority
/// through `root`.
async fn postgres_refusing_every_statement(authority: &TestAuthority, root: &Path) -> String {
    let mut body = Vec::new();
    for (kind, value) in [
        (b'S', "ERROR"),
        (b'V', "ERROR"),
        (b'C', "22P02"),
        (
            b'M',
            "invalid input syntax for type uuid: \"zzqx-ada@example.com\"",
        ),
    ] {
        body.push(kind);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    let ready = postgres_message(b'Z', b"I");
    let mut refusal = postgres_message(b'E', &body);
    refusal.extend_from_slice(&ready);
    let mut welcome = postgres_message(b'R', &0_u32.to_be_bytes());
    welcome.extend_from_slice(&ready);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
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
    format!(
        "postgres://nestrs@127.0.0.1:{port}/postgres?sslmode=verify-full&sslrootcert={}",
        root.display()
    )
}

/// What the fixture meets creating its database is said by its SQLSTATE: no
/// reader of its error prints the value the server's message quoted.
#[tokio::test]
async fn a_refusal_the_fixture_meets_names_no_value_the_server_quoted() {
    let authority = TestAuthority::new();
    let root = tempfile::NamedTempFile::new().expect("a file for the authority");
    std::fs::write(root.path(), authority.pem()).expect("the authority's certificate");
    let url = postgres_refusing_every_statement(&authority, root.path()).await;

    let Err(refused) = EphemeralDatabase::create_with::<NoMigrations>(&url).await else {
        panic!("the fixture created a database on a server refusing every statement");
    };

    let printed = nest_rs_testing::printed(&*refused);
    assert!(printed.contains("22P02"), "said by its SQLSTATE: {printed}");
    assert!(!printed.contains(MARKED), "{printed}");
}
