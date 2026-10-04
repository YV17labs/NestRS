//! The connection's doubles and the refusal both suites assert: a server that
//! scripts a not-ready Redis, and the shape of a refusal that came at once.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError};

use super::AT_ONCE;

/// A budget far above [`AT_ONCE`], so a refusal that retried would show.
pub(crate) fn config(url: String) -> RedisConfig {
    RedisConfig {
        url,
        connect_timeout: Duration::from_secs(10),
        ..RedisConfig::default()
    }
}

/// What Redis answered, which the refusal carries as its source.
pub(crate) fn answer(error: &RedisError) -> String {
    std::error::Error::source(error)
        .map(ToString::to_string)
        .unwrap_or_default()
}

/// Connect to `url`, whose database `index` Redis will not select, and return
/// the refusal once it is shown to have come at once, naming the index and the
/// variable that holds it.
pub(crate) async fn database_refused_at_once(url: String, index: i64, case: &str) -> RedisError {
    let started = Instant::now();
    let Err(error) = RedisConnection::connect(&config(url)).await else {
        panic!("{case} must not connect")
    };
    let took = started.elapsed();
    assert!(
        took < AT_ONCE,
        "{case}: a refusal spends none of the budget, took {took:?}"
    );
    assert!(
        matches!(&error, RedisError::DatabaseRefused { database, .. } if *database == index),
        "{case}: {error}"
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains(&format!("refused to select database {index}"))
            && rendered.contains(&nest_rs_config::var_name("redis", "URL")),
        "{case}: the error names the index and the variable that holds it: {rendered}",
    );
    error
}

/// A server playing the side of two states the boot must tell apart from a
/// refusal: while an answer is set, every connection it accepts is answered
/// with that error line for every command — Redis busy running a script,
/// loading its dataset, failing over — and with `slots`, a connection past that
/// many forwarded at once is refused the way Redis refuses one past
/// `maxclients`. Anything else is forwarded to `upstream`, or closed when there
/// is none: an in-process test names no Redis, so it cannot dial one.
pub(crate) struct ScriptedRedis {
    addr: SocketAddr,
    pub(crate) answer: Arc<Mutex<Option<&'static str>>>,
}

impl ScriptedRedis {
    pub(crate) async fn start(upstream: Option<String>, slots: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let answer: Arc<Mutex<Option<&'static str>>> = Arc::new(Mutex::new(None));
        let answering = Arc::clone(&answer);
        let open = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let scripted = *answering.lock().expect("answer lock");
                if let Some(line) = scripted {
                    tokio::spawn(answer_every_command(client, line));
                    continue;
                }
                if slots.is_some_and(|slots| open.load(Ordering::SeqCst) >= slots) {
                    tokio::spawn(async move {
                        let _ = client
                            .write_all(b"-ERR max number of clients reached\r\n")
                            .await;
                    });
                    continue;
                }
                open.fetch_add(1, Ordering::SeqCst);
                let open = Arc::clone(&open);
                let upstream = upstream.clone();
                tokio::spawn(async move {
                    if let Some(upstream) = upstream
                        && let Ok(mut server) = TcpStream::connect(&upstream).await
                    {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                    }
                    open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self { addr, answer }
    }

    pub(crate) fn url(&self) -> String {
        format!("redis://{}/", self.addr)
    }

    /// Answer every command on connections accepted from now with `line`, or
    /// forward them again with `None`.
    pub(crate) fn answer_with(&self, line: Option<&'static str>) {
        *self.answer.lock().expect("answer lock") = line;
    }
}

/// Answer each command `client` sends with the error `line`, until it hangs up.
async fn answer_every_command(mut client: TcpStream, line: &'static str) {
    let reply = format!("-{line}\r\n");
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        while let Some(length) = command_length(&received) {
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
}

/// The length of the first whole command in `bytes` — an array of bulk
/// strings, the only form the client sends — or `None` until it has arrived.
fn command_length(bytes: &[u8]) -> Option<usize> {
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
    for _ in 0..arguments {
        let (body, length) = header(at, b'$')?;
        at = body + length + 2;
    }
    (at <= bytes.len()).then_some(at)
}

/// What a Redis answers while it is not ready yet — the transient codes the
/// client knows, and one it does not.
pub(crate) const NOT_READY: [&str; 5] = [
    "BUSY Redis is busy running a script. You can only call SCRIPT KILL or SHUTDOWN NOSAVE.",
    "LOADING Redis is loading the dataset in memory",
    "MASTERDOWN Link with MASTER is down and replica-serve-stale-data is set to 'no'.",
    "TRYAGAIN Multiple keys request during rehashing of slot",
    "SOMEDAYCODE an answer this client has never heard of",
];
