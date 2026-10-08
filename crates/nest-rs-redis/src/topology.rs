//! [`RedisTopology`] — how a deployment lays its Valkey out, as the scheme of
//! `<PREFIX>_REDIS__URL` declares it — and [`Hello`], what a server says it
//! serves.
//!
//! **`HELLO` is how a connection checks what it reached**: the ACL never
//! governs it, so every user may send it, and its `mode` and `role` say it
//! where a refused command's text would only hint at it.

use std::collections::HashMap;
use std::fmt;

use redis::aio::ConnectionLike;
use redis::{ErrorKind, Value};

/// A deployment topology Valkey offers in production, named as its servers
/// report their mode. The URL's scheme declares it, and nothing infers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RedisTopology {
    /// One primary, with or without replicas, reached at one address
    /// (`redis://`, `rediss://`).
    Standalone,
    /// A primary the sentinels name, found again through them after every
    /// failover (`redis-sentinel://`, `rediss-sentinel://`).
    Sentinel,
    /// Primaries sharded by hash slot, each command sent to the node holding
    /// its keys (`redis-cluster://`, `rediss-cluster://`).
    Cluster,
}

impl fmt::Display for RedisTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standalone => "standalone",
            Self::Sentinel => "Sentinel",
            Self::Cluster => "Cluster",
        })
    }
}

/// What a server's `HELLO` says of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hello {
    /// The topology the server serves.
    pub(crate) serves: RedisTopology,
    /// Whether it takes writes.
    pub(crate) primary: bool,
}

impl Hello {
    /// Ask the server behind `connection`, keeping the protocol it speaks.
    pub(crate) async fn ask(
        connection: &mut impl ConnectionLike,
    ) -> Result<Self, redis::RedisError> {
        let reply: HashMap<String, Value> =
            redis::cmd("HELLO").arg(2).query_async(connection).await?;
        Self::read(&reply)
    }

    fn read(reply: &HashMap<String, Value>) -> Result<Self, redis::RedisError> {
        let field = |name: &str| {
            reply
                .get(name)
                .and_then(|value| redis::from_redis_value_ref::<String>(value).ok())
        };
        let serves = match field("mode").as_deref() {
            Some("standalone") => RedisTopology::Standalone,
            Some("sentinel") => RedisTopology::Sentinel,
            Some("cluster") => RedisTopology::Cluster,
            _ => {
                return Err(redis::RedisError::from((
                    ErrorKind::UnexpectedReturnType,
                    "HELLO names no mode Valkey serves",
                )));
            }
        };
        Ok(Self {
            serves,
            primary: matches!(field("role").as_deref(), Some("master" | "primary")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(mode: &str, role: &str) -> HashMap<String, Value> {
        [("mode", mode), ("role", role)]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), Value::BulkString(value.into())))
            .collect()
    }

    #[test]
    fn hello_says_the_topology_a_server_serves_and_whether_it_is_a_primary() {
        for (mode, role, serves, primary) in [
            ("standalone", "master", RedisTopology::Standalone, true),
            ("standalone", "replica", RedisTopology::Standalone, false),
            ("sentinel", "sentinel", RedisTopology::Sentinel, false),
            ("cluster", "master", RedisTopology::Cluster, true),
            ("cluster", "replica", RedisTopology::Cluster, false),
        ] {
            assert_eq!(
                Hello::read(&reply(mode, role)).expect("a mode Valkey serves"),
                Hello { serves, primary },
                "{mode} {role}",
            );
        }
    }

    #[test]
    fn a_hello_naming_no_known_mode_is_refused() {
        assert!(Hello::read(&reply("proxy", "master")).is_err());
        assert!(Hello::read(&HashMap::new()).is_err());
    }
}
