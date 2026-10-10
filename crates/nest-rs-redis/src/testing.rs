//! What the unit tests of every link stand in front of: certificates the test
//! process issues, a TLS listener presenting one, a listener that never
//! answers, and an answer as Valkey sends it.

use std::sync::LazyLock;
use std::time::Duration;

use nest_rs_testing::{TestAuthority, TestCertificate};

use crate::RedisConfig;

/// The authority this test process issues its certificates with.
pub(crate) static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

/// What the TLS listener presents: the loopback it listens on.
pub(crate) static SERVER: LazyLock<TestCertificate> =
    LazyLock::new(|| AUTHORITY.server(&["127.0.0.1", "localhost"]));

/// A client certificate of the same authority.
pub(crate) static CLIENT: LazyLock<TestCertificate> =
    LazyLock::new(|| AUTHORITY.client("nestrs-test-client"));

/// `url` under `budget`, everything else at its default.
pub(crate) fn config(url: &str, budget: Duration) -> RedisConfig {
    RedisConfig {
        url: url.to_owned(),
        connect_timeout: budget,
        ..RedisConfig::default()
    }
}

/// A TLS listener presenting a certificate the test authority issued —
/// nothing a client trusts by default — and answering nothing past the
/// handshake.
pub(crate) async fn tls_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let acceptor = SERVER.acceptor(None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a TLS listener");
    let addr = listener.local_addr().expect("the listener's address");
    let serving = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "the test listener serves whoever connects; a failed handshake is the client's assertion"
                )]
                let _ = acceptor.accept(socket).await;
            });
        }
    });
    (addr, serving)
}

/// A TLS listener requiring a client certificate the test authority issued, as
/// Valkey with `tls-auth-clients yes` does, and answering nothing past the
/// handshake. A refused handshake ends as a TLS server ends it: what the
/// client sent after its alert is read before the socket closes, since a
/// socket dropped with data unread answers with a reset that can overtake the
/// alert.
pub(crate) async fn mutual_tls_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let acceptor = SERVER.acceptor(Some(&AUTHORITY));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a TLS listener");
    let addr = listener.local_addr().expect("the listener's address");
    let serving = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Err((_, mut refused)) = acceptor.accept(socket).into_fallible().await {
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "a client already gone has nothing left to read"
                    )]
                    let _ = refused.shutdown().await;
                    let mut sink = [0u8; 1024];
                    while refused.read(&mut sink).await.is_ok_and(|read| read > 0) {}
                }
            });
        }
    });
    (addr, serving)
}

/// A listener that accepts and never answers: the connection opens, and the
/// round trip that would prove it never returns.
pub(crate) async fn silent_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a silent listener");
    let addr = listener.local_addr().expect("the listener's address");
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    (addr, silent)
}

/// An answer as Valkey sends it, parsed as the client parses one.
pub(crate) fn answer(line: &str) -> redis::RedisError {
    let value =
        redis::parse_redis_value(format!("-{line}\r\n").as_bytes()).expect("a RESP error parses");
    let redis::Value::ServerError(error) = value else {
        panic!("a RESP error parses as a server error");
    };
    error.into()
}

/// Asserts every key of `keys` is a level of the concern `target` emits on —
/// `nestrs:<concern>:<structure>[:…]`, `<concern>` the target's tail, never
/// `redis` — and that none prefixes another inside a level, which a `SCAN`
/// matching by glob would read as one.
pub(crate) fn assert_keys_of(target: &str, keys: &[&str]) {
    let concern = target
        .strip_prefix("nest_rs::")
        .expect("a framework target")
        .replace("::", ":");
    let level = format!("nestrs:{concern}:");
    for (at, key) in keys.iter().enumerate() {
        assert!(
            key.starts_with(&level),
            "{key} is `nestrs:{concern}:<structure>`"
        );
        for (other_at, other) in keys.iter().enumerate() {
            if other_at != at
                && let Some(rest) = other.strip_prefix(key)
            {
                assert!(
                    rest.starts_with(':'),
                    "{key} prefixes {other} inside a level"
                );
            }
        }
    }
}
