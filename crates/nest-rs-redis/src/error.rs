//! Typed errors for the Redis substrate.
//!
//! Framework crates surface `thiserror` enums, not `anyhow`. Opening the shared
//! connection is a Redis-specific step, so it carries its own error here; the
//! producer surface (the `JobProducer` impl on
//! [`RedisQueueProducer`](crate::RedisQueueProducer)) instead speaks the
//! backend-agnostic
//! [`QueueError`](::nest_rs_queue::QueueError), wrapping a Redis push failure as
//! its opaque `Backend` source.

use thiserror::Error;

/// A failure opening the shared [`RedisConnection`](crate::RedisConnection)
/// from its configuration.
///
/// Concern-prefixed (`RedisError`, not a generic `ConnectionError`) to match
/// the house pattern — `ConfigError`, `StorageError`, `QueueError` — and avoid
/// a name collision when an app imports several infra errors at once.
///
/// Every variant names the endpoint as the address the client dials — never the
/// URL, which may embed a password, and every one of these strings reaches logs
/// and stderr.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RedisError {
    /// The URL could not configure a connection — malformed, or a scheme the
    /// client does not speak. It fails the same way on every attempt, so it is
    /// refused at once rather than retried for the whole budget.
    #[error(
        "invalid Redis URL {endpoint}: check {url_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
    )]
    InvalidUrl {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// Why the client rejected it.
        #[source]
        source: redis::RedisError,
    },

    /// The connect budget is outside the range its variable is held to — a
    /// [`RedisConfig`](crate::RedisConfig) built in code and handed to
    /// [`RedisConnection::connect`](crate::RedisConnection::connect) without a
    /// config read. A zero budget would give up before the first attempt and
    /// then blame the URL, so it is refused before anything is dialled, in the
    /// words the variable is refused in.
    #[error(transparent)]
    Budget(nest_rs_config::ConfigError),

    /// A `rediss://` URL carrying `#insecure`, which asks the client to accept
    /// whatever certificate it is shown. Encryption nobody verified is open to
    /// whoever sits between the app and Redis, so it is refused rather than
    /// honoured: a certificate a private authority signed is trusted by
    /// configuring that authority.
    #[error(
        "the Redis URL for {endpoint} asks to skip certificate verification (`#insecure`), which is \
         refused: remove it from the URL — {url_var}, or `RedisConfig::url` pinned in code — and \
         trust the authority that signed Redis's certificate with {ca_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
        ca_var = ::nest_rs_config::spellings("redis", "TLS_CA_CERT"),
    )]
    UnverifiedTls {
        /// The address the client dials, never the URL.
        endpoint: String,
    },

    /// TLS material configured beside a URL that is not `rediss://`. The
    /// connection would be plaintext and the material unused, while whoever
    /// configured an authority or a client certificate expects it to be — so
    /// it is refused rather than ignored.
    #[error(
        "TLS material is configured for Redis at {endpoint}, but the URL is not `rediss://`, so the \
         material would go unused and the connection unencrypted: point {url_var} at a port serving \
         TLS with `rediss://`, or remove the material — {ca_var}, {cert_var} and {key_var}, their \
         _FILE forms, or `RedisConfig::tls` pinned in code",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
        ca_var = ::nest_rs_config::var_name("redis", "TLS_CA_CERT"),
        cert_var = ::nest_rs_config::var_name("redis", "TLS_CERT"),
        key_var = ::nest_rs_config::var_name("redis", "TLS_KEY"),
    )]
    PlaintextUrl {
        /// The address the client dials, never the URL.
        endpoint: String,
    },

    /// The TLS settings could not carry a connection: material no handshake
    /// could use, a certificate the client does not accept, a handshake Redis
    /// refused, or a peer answering in something other than TLS. The same
    /// settings fail the same way on every attempt, and none of it is an
    /// outage, so it fails at once with what to change: read off rustls's reason
    /// when a handshake refused, or off the material check before anything was
    /// dialled.
    #[error("TLS with Redis at {endpoint} was refused: {reason}")]
    TlsRefused {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// What to change, naming the setting. When a handshake is what
        /// refused, rustls's reason travels in `source`.
        reason: String,
        /// The client's error, when the refusal came from a connection attempt
        /// or from building the client rather than from checking the material.
        #[source]
        source: Option<redis::RedisError>,
    },

    /// Redis answered, and refused in a way every attempt would repeat —
    /// credentials, an ACL denying the proof, a protocol it does not speak.
    /// That is not an outage, and retrying it would only tell the operator to
    /// look at the network — so it fails at once, naming the variable that
    /// holds the URL, with Redis's answer as the source.
    #[error(
        "Redis at {endpoint} refused the connection: check {url_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
    )]
    Refused {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// What Redis answered.
        #[source]
        source: redis::RedisError,
    },

    /// Redis refused to select the database the URL names, for a reason every
    /// attempt would repeat: an index past the server's `databases`, an ACL
    /// user without `+select`, a server in cluster mode, which serves database 0
    /// alone. It fails at once naming the index, with Redis's answer as the
    /// source.
    ///
    /// Its own variant rather than [`Refused`](Self::Refused) because the client
    /// reports every refused `SELECT` under one sentence and drops the server's
    /// code — `NOPERM`, `ERR` — so the index is the one fact the operator
    /// cannot read off the answer. The only answer to a `SELECT` that clears is
    /// a server busy running a script; that one is retried within the budget.
    #[error(
        "Redis at {endpoint} refused to select database {database}, the index {url_var} ends \
         in — its answer follows: the index must be below the server's `databases`, an ACL \
         user needs `+select`, and a server in cluster mode serves database 0 alone",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
    )]
    DatabaseRefused {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// The database index the URL names.
        database: i64,
        /// What Redis answered.
        #[source]
        source: redis::RedisError,
    },

    /// The connect budget elapsed with Redis answering every attempt, last with
    /// an answer that may clear — a dataset still loading, a script past its
    /// threshold, a failover in progress, a code this client does not know.
    /// Redis was reached, so the sentence does not send the operator to the
    /// URL: the answer travels as the source, and the budget is what to widen
    /// if it clears on its own.
    #[error(
        "Redis at {endpoint} answered but was not ready within {budget:?} ({attempts} \
         attempt(s)) — its last answer follows; widen the budget with {timeout_var} if it \
         clears on its own",
        timeout_var = ::nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
    )]
    Unready {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// The budget that elapsed.
        budget: std::time::Duration,
        /// How many connect attempts were made inside it.
        attempts: u32,
        /// Redis's last answer.
        #[source]
        source: redis::RedisError,
    },

    /// The connect budget elapsed with the backend still unreachable. Carries
    /// the redacted endpoint and the knob to widen, because this is the boot
    /// error an operator reads at 3am — an unreachable queue used to be
    /// indistinguishable from a hung process.
    #[error(
        "could not reach Redis at {endpoint} within {budget:?} ({attempts} attempt(s)): \
         check {url_var}, or widen the budget with {timeout_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
        timeout_var = ::nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
    )]
    Unreachable {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// The budget that elapsed.
        budget: std::time::Duration,
        /// How many connect attempts were made inside it.
        attempts: u32,
        /// The last transport failure, when the budget ran out after an
        /// outright error rather than mid-attempt.
        #[source]
        source: Option<redis::RedisError>,
    },
}

/// A queue this worker serves still holds jobs under the 6.x key layout — at the
/// root of the keyspace, under the queue's bare name, where a 7.0 worker never
/// looks. Starting beside them would leave them waiting with nothing to say so,
/// which is why the boot refuses instead and names the two ways out.
///
/// Not public: it reaches the operator as the boot's error, and nothing matches
/// on it.
#[derive(Debug, Error)]
#[error(
    "queue `{queue}` still holds jobs under the 6.x key layout ({keys}), which this worker never \
     reads, so it does not start beside them. Drain them first: run a 6.x worker until those keys \
     are gone. Or move them, with every worker of the queue stopped and before anything is pushed \
     under `{namespace}`: `RENAMENX` each of the queue's 6.x keys to the same structure under \
     `{namespace}` — the in-flight set takes one step more, listed with the rest on the queue \
     documentation's \"Upgrading queues from 6.x\" page. If those keys are an application's \
     own rather than a 6.x queue's, rename them out of the way instead: moved under \
     `{namespace}`, they would be read as jobs"
)]
pub(crate) struct LegacyLayoutError {
    /// The queue whose jobs wait under the old layout.
    pub(crate) queue: String,
    /// What the old layout holds: each structure, and how many jobs wait there
    /// — comma-separated.
    pub(crate) keys: String,
    /// Where the queue lives now.
    pub(crate) namespace: String,
}
