//! What the unit tests of every link stand in front of: certificates the test
//! process issues, a TLS listener presenting one, a listener that never
//! answers, and an answer as Valkey sends it.

use std::sync::LazyLock;
use std::time::Duration;

use nest_rs_config::{ClientTls, Material, TlsIdentity};
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

/// `pem` as a value given inline.
pub(crate) fn inline(pem: &str) -> Material {
    Material {
        bytes: pem.as_bytes().to_vec(),
        path: None,
    }
}

/// The test authority trusted, and no certificate presented.
pub(crate) fn trusting_the_authority() -> ClientTls {
    ClientTls::new(Some(inline(AUTHORITY.pem())), None)
}

/// `cert` presented with `key`, nothing else set.
pub(crate) fn presenting(cert: &str, key: &str) -> ClientTls {
    ClientTls::new(None, Some(TlsIdentity::new(inline(cert), inline(key))))
}

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
/// handshake.
pub(crate) async fn mutual_tls_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let acceptor = SERVER.acceptor(Some(&AUTHORITY));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a TLS listener");
    let addr = listener.local_addr().expect("the listener's address");
    let serving = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(refuse_an_uncertified_client(acceptor.clone(), socket));
        }
    });
    (addr, serving)
}

/// A [`mutual_tls_listener`] whose refusal lands before the client's first
/// command, as a server faster than the client lands it, on every run: its
/// first connection completes the handshake and is closed at once — the drop
/// `redis` turns a refusal read before a command into — and every later one is
/// refused.
pub(crate) async fn dropping_then_refusing_listener()
-> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;

    let accepting = SERVER.acceptor(None);
    let refusing = SERVER.acceptor(Some(&AUTHORITY));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a TLS listener");
    let addr = listener.local_addr().expect("the listener's address");
    let serving = tokio::spawn(async move {
        let Ok((first, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            if let Ok(mut dropped) = accepting.accept(first).await {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a client already gone has met the drop all the same"
                )]
                let _ = dropped.shutdown().await;
            }
        });
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(refuse_an_uncertified_client(refusing.clone(), socket));
        }
    });
    (addr, serving)
}

/// The handshake `acceptor` refuses ended as a TLS server ends it: what the
/// client sent after its alert is read before the socket closes, since a socket
/// dropped with data unread answers with a reset that can overtake the alert.
async fn refuse_an_uncertified_client(
    acceptor: tokio_rustls::TlsAcceptor,
    socket: tokio::net::TcpStream,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    if let Err((_, mut refused)) = acceptor.accept(socket).into_fallible().await {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a client already gone has nothing left to read"
        )]
        let _ = refused.shutdown().await;
        let mut sink = [0u8; 1024];
        while refused.read(&mut sink).await.is_ok_and(|read| read > 0) {}
    }
}

/// A sentinel over TLS, presenting what [`tls_listener`] presents, that names
/// `primary` as the primary of every service it is asked about — enough of the
/// protocol for a link to ask it, and nothing else.
pub(crate) async fn sentinel_listener(
    primary: std::net::SocketAddr,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let acceptor = SERVER.acceptor(None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a sentinel listener");
    let addr = listener.local_addr().expect("the listener's address");
    let named = primary.port().to_string();
    let answers = move |command: &[u8]| -> String {
        match command.to_ascii_uppercase().as_slice() {
            b"HELLO" => {
                "*4\r\n$4\r\nmode\r\n$8\r\nsentinel\r\n$4\r\nrole\r\n$8\r\nsentinel\r\n".to_owned()
            }
            b"SENTINEL" => format!("*2\r\n$9\r\n127.0.0.1\r\n${}\r\n{named}\r\n", named.len()),
            _ => "+OK\r\n".to_owned(),
        }
    };
    let serving = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let answers = answers.clone();
            tokio::spawn(async move {
                let Ok(mut client) = acceptor.accept(socket).await else {
                    return;
                };
                let mut received = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    while let Some((length, command)) = first_command(&received) {
                        let reply = answers(&command);
                        received.drain(..length);
                        if client.write_all(reply.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    match client.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => received.extend_from_slice(&chunk[..read]),
                    }
                }
            });
        }
    });
    (addr, serving)
}

/// The length of the first whole command in `bytes` — an array of bulk
/// strings, the only form the client sends — and its name, or `None` until it
/// has arrived.
fn first_command(bytes: &[u8]) -> Option<(usize, Vec<u8>)> {
    let header = |from: usize, marker: u8| -> Option<(usize, usize)> {
        if bytes.get(from) != Some(&marker) {
            return None;
        }
        let end = from
            + bytes
                .get(from..)?
                .windows(2)
                .position(|pair| pair == b"\r\n")?;
        let number = std::str::from_utf8(&bytes[from + 1..end])
            .ok()?
            .parse()
            .ok()?;
        Some((end + 2, number))
    };
    let (mut at, arguments) = header(0, b'*')?;
    let mut name = None;
    for _ in 0..arguments {
        let (body, length) = header(at, b'$')?;
        at = body + length + 2;
        if name.is_none() {
            name = Some(bytes.get(body..body + length)?.to_vec());
        }
    }
    (at <= bytes.len()).then_some((at, name?))
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
