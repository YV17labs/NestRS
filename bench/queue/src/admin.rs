//! The few commands the bench sends Redis itself — clearing its own keys
//! between runs and reading the counters — over plain RESP, so measuring never
//! depends on how the adapter under test talks to Redis.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

use nest_rs::queue::Queue;

use crate::command::{C1Queue, C16Queue};

const ANSWER_WITHIN: Duration = Duration::from_secs(10);

pub struct Admin {
    addr: String,
    db: String,
}

/// The commands the bench itself sends, left out of what it reports.
const OWN: [&str; 5] = ["info", "config", "select", "scan", "unlink"];

/// The queues the bench files jobs on: every key it owns is one of theirs.
const QUEUES: [&str; 2] = [<C1Queue as Queue>::NAME, <C16Queue as Queue>::NAME];

/// Keys read or deleted per round trip.
const PAGE: &str = "1000";

/// Redis's counters at one instant. A command a script runs counts as one, as
/// the script does: Redis counts them both.
pub struct Counters {
    pub cpu: Duration,
    pub by_command: BTreeMap<String, u64>,
}

impl Admin {
    /// Plain `redis://host[:port][/db]` only: the bench measures, it does not
    /// carry the adapter's TLS or credentials.
    pub fn from_url(url: &str) -> Result<Self> {
        let Some(rest) = url.strip_prefix("redis://") else {
            bail!("the bench reaches Redis at a plain redis://host:port/db URL");
        };
        if rest.contains('@') {
            bail!("the bench reaches Redis without credentials");
        }
        let (host, db) = rest.split_once('/').unwrap_or((rest, ""));
        let addr = if host.contains(':') {
            host.to_string()
        } else {
            format!("{host}:6379")
        };
        let db = if db.is_empty() { "0" } else { db }.to_string();
        Ok(Self { addr, db })
    }

    pub async fn version(&self) -> Result<String> {
        let info = self.text(&["INFO", "server"]).await?;
        field(&info, "redis_version").context("INFO server names no redis_version")
    }

    /// Refuse a Redis holding anything but the bench's own keys: the bench
    /// deletes between runs and reads server-wide counters, so a Redis it shares
    /// would lose data and skew every figure.
    pub async fn claim(&self) -> Result<()> {
        let keyspace = self.text(&["INFO", "keyspace"]).await?;
        for line in keyspace.lines() {
            let Some((db, stats)) = line.strip_prefix("db").and_then(|l| l.split_once(':')) else {
                continue;
            };
            let keys = stats
                .split(',')
                .find_map(|kv| kv.strip_prefix("keys="))
                .unwrap_or("0");
            if db != self.db && keys != "0" {
                bail!(
                    "this Redis holds {keys} key(s) in database {db}: the bench deletes keys and \
                     reads counters across the server, so give it a Redis of its own"
                );
            }
        }
        let foreign = self
            .keys("*")
            .await?
            .into_iter()
            .filter(|key| !is_own(key))
            .count();
        if foreign > 0 {
            bail!(
                "database {} holds {foreign} key(s) the bench did not file: the bench deletes \
                 keys and reads counters across the server, so give it a Redis of its own",
                self.db
            );
        }
        Ok(())
    }

    /// Delete every key the bench's queues left, so each run starts with no job.
    pub async fn clear(&self) -> Result<()> {
        let own: Vec<String> = self
            .keys("nestrs:queue:*")
            .await?
            .into_iter()
            .filter(|key| is_own(key))
            .collect();
        for page in own.chunks(1000) {
            let mut args = vec!["UNLINK"];
            args.extend(page.iter().map(String::as_str));
            self.call(&args).await?;
        }
        Ok(())
    }

    /// Every key matching `pattern` in the bench's database.
    async fn keys(&self, pattern: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut cursor = String::from("0");
        loop {
            let Reply::Array(mut answer) = self
                .call(&["SCAN", &cursor, "MATCH", pattern, "COUNT", PAGE])
                .await?
            else {
                bail!("SCAN answered something other than an array");
            };
            let (Some(Reply::Array(page)), Some(Reply::Bulk(next))) = (answer.pop(), answer.pop())
            else {
                bail!("SCAN answered an array of another shape");
            };
            keys.extend(page.into_iter().filter_map(|key| match key {
                Reply::Bulk(key) => Some(key),
                _ => None,
            }));
            if next == "0" {
                return Ok(keys);
            }
            cursor = next;
        }
    }

    pub async fn counters(&self) -> Result<Counters> {
        let cpu = self.text(&["INFO", "cpu"]).await?;
        let seconds = |name: &str| -> Result<f64> {
            field(&cpu, name)
                .with_context(|| format!("INFO cpu names no {name}"))?
                .parse::<f64>()
                .with_context(|| format!("INFO cpu's {name} is not a number"))
        };
        let cpu = Duration::from_secs_f64(seconds("used_cpu_user")? + seconds("used_cpu_sys")?);
        let by_command = self
            .text(&["INFO", "commandstats"])
            .await?
            .lines()
            .filter_map(|line| {
                let (name, rest) = line.strip_prefix("cmdstat_")?.split_once(':')?;
                if OWN.contains(&name.split('|').next()?) {
                    return None;
                }
                let calls = rest.split(',').find_map(|kv| kv.strip_prefix("calls="))?;
                Some((name.to_string(), calls.parse().ok()?))
            })
            .collect();
        Ok(Counters { cpu, by_command })
    }

    async fn text(&self, args: &[&str]) -> Result<String> {
        match self.call(args).await? {
            Reply::Bulk(text) => Ok(text),
            Reply::Line(line) => bail!("{} answered `{line}`, not a bulk string", args[0]),
            Reply::Array(_) => bail!("{} answered an array, not a bulk string", args[0]),
        }
    }

    async fn call(&self, args: &[&str]) -> Result<Reply> {
        timeout(ANSWER_WITHIN, self.exchange(args))
            .await
            .with_context(|| format!("Redis did not answer {} within {ANSWER_WITHIN:?}", args[0]))?
    }

    async fn exchange(&self, args: &[&str]) -> Result<Reply> {
        let mut stream = BufReader::new(
            TcpStream::connect(&self.addr)
                .await
                .with_context(|| format!("connecting to Redis at {}", self.addr))?,
        );
        stream
            .write_all(&encode(&["SELECT", self.db.as_str()]))
            .await?;
        stream.write_all(&encode(args)).await?;
        read(&mut stream).await?;
        read(&mut stream).await
    }
}

enum Reply {
    Line(String),
    Bulk(String),
    Array(Vec<Reply>),
}

/// Whether `key` belongs to one of the bench's queues, under any layout an
/// adapter files them in (`nestrs:queue:<queue>:…` or `nestrs:queue:{<queue>}:…`).
fn is_own(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("nestrs:queue:") else {
        return false;
    };
    QUEUES.iter().any(|queue| {
        let bare = rest.strip_prefix(queue);
        let tagged = rest
            .strip_prefix('{')
            .and_then(|rest| rest.strip_prefix(queue))
            .and_then(|rest| rest.strip_prefix('}'));
        bare.or(tagged).is_some_and(|tail| tail.starts_with(':'))
    })
}

fn encode(args: &[&str]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n{arg}\r\n", arg.len()).as_bytes());
    }
    out
}

fn read(
    stream: &mut BufReader<TcpStream>,
) -> Pin<Box<dyn Future<Output = Result<Reply>> + Send + '_>> {
    Box::pin(async move {
        let mut line = String::new();
        stream.read_line(&mut line).await?;
        let line = line.trim_end();
        match line.split_at_checked(1) {
            Some(("+" | ":", rest)) => Ok(Reply::Line(rest.to_string())),
            Some(("-", error)) => bail!("Redis refused the command: {error}"),
            Some(("$", len)) => {
                let len: usize = len.parse().context("a bulk length that is not a number")?;
                let mut body = vec![0; len + 2];
                stream.read_exact(&mut body).await?;
                body.truncate(len);
                // A key the bench did not file may be any bytes; it is only
                // ever compared with the bench's own names.
                Ok(Reply::Bulk(String::from_utf8_lossy(&body).into_owned()))
            }
            Some(("*", len)) => {
                let len: usize = len
                    .parse()
                    .context("an array length that is not a number")?;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(read(stream).await?);
                }
                Ok(Reply::Array(items))
            }
            _ => bail!("an answer the bench does not read: `{line}`"),
        }
    })
}

fn field(info: &str, name: &str) -> Option<String> {
    info.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .map(|value| value.trim().to_string())
}
