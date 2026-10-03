//! The few commands the bench sends Redis itself — flushing between runs and
//! reading its counters — over plain RESP, so measuring never depends on how
//! the adapter under test talks to Redis.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

const ANSWER_WITHIN: Duration = Duration::from_secs(10);

pub struct Admin {
    addr: String,
    db: String,
}

/// The commands the bench itself sends, left out of what it reports.
const OWN: [&str; 4] = ["info", "config", "select", "flushdb"];

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

    /// Empty the database the URL names, so each run starts with no job.
    pub async fn flush(&self) -> Result<()> {
        self.call(&["FLUSHDB"]).await.map(drop)
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
}

fn encode(args: &[&str]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n{arg}\r\n", arg.len()).as_bytes());
    }
    out
}

async fn read(stream: &mut BufReader<TcpStream>) -> Result<Reply> {
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
            Ok(Reply::Bulk(String::from_utf8(body)?))
        }
        _ => bail!("an answer the bench does not read: `{line}`"),
    }
}

fn field(info: &str, name: &str) -> Option<String> {
    info.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .map(|value| value.trim().to_string())
}
