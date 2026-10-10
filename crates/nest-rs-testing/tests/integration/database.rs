//! Covers `src/database.rs`: the fixture opens its connections as the app's
//! pool does — TLS verified, never encrypted without a check nor plaintext
//! because a server declined it.

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
