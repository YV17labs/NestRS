//! [`RedisTopology`] — how a deployment lays its Valkey out, as the scheme of
//! `<PREFIX>_REDIS__URL` declares it.

use std::fmt;

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
