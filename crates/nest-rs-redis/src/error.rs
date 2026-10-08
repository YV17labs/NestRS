//! Typed errors for the Redis substrate. The queue producer speaks
//! [`QueueError`](::nest_rs_queue::QueueError) instead, wrapping a Redis failure
//! as its opaque `Backend` source.

use thiserror::Error;

/// A failure opening the shared [`RedisConnection`](crate::RedisConnection)
/// from its configuration, or binding a port over it.
///
/// Every variant names the endpoint as the address the client dials — never the
/// URL, which may embed a password, and every one of these strings reaches logs
/// and stderr.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RedisError {
    /// The URL could not configure a connection — malformed, or a scheme the
    /// client does not speak. Refused at once, never retried.
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

    /// A URL asking for RESP3 (`protocol=resp3`): every binding reads Redis's
    /// RESP2 replies.
    #[error(
        "the Redis URL for {endpoint} asks for RESP3 (`protocol=resp3`), which nestrs does not \
         speak to Redis: remove it from {url_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
    )]
    UnsupportedProtocol {
        /// The address the client dials, never the URL.
        endpoint: String,
    },

    /// The connect budget is outside the range its variable is held to — a
    /// [`RedisConfig`](crate::RedisConfig) built in code and handed to
    /// [`RedisConnection::connect`](crate::RedisConnection::connect) without a
    /// config read. Refused before anything is dialled, in the words the variable
    /// is refused in.
    #[error(transparent)]
    Budget(nest_rs_config::ConfigError),

    /// A queue lease the connect budget leaves no renewal room in: a renewal
    /// sent a third in may wait out the whole budget.
    #[error(
        "the Redis queue lease ({lease:?}) must be more than one and a half times the Redis \
         budget ({budget:?}): a renewal is sent a third into the lease and may wait out the \
         whole budget, so a shorter lease lapses while Redis still answers — raise {lease_var} \
         or lower {timeout_var}",
        lease_var = ::nest_rs_config::var_name("redis__queue", crate::queue::LEASE.key()),
        timeout_var = ::nest_rs_config::var_name("redis", crate::config::CONNECT_TIMEOUT.key()),
    )]
    BudgetPastLease {
        /// The connect budget.
        budget: std::time::Duration,
        /// The queue binding's lease.
        lease: std::time::Duration,
    },

    /// A `rediss://` URL carrying `#insecure`, which asks the client to accept
    /// whatever certificate it is shown — refused; a private authority is
    /// trusted by configuring it.
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

    /// TLS material configured beside a URL that is not `rediss://`, where it
    /// would go unused — refused rather than ignored.
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
    /// refused, or a peer answering in something other than TLS. Fails at once
    /// with what to change.
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
    /// credentials, an ACL denying the proof, a protocol it does not speak. Fails
    /// at once, with Redis's answer as the source.
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
    #[error(
        "Redis at {endpoint} answered but was not ready within {budget:?} ({attempts} \
         attempt(s)) — its last answer follows; widen the budget with {timeout_var} if it \
         clears on its own",
        timeout_var = ::nest_rs_config::var_name("redis", crate::config::CONNECT_TIMEOUT.key()),
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

    /// The connect budget elapsed with the backend still unreachable.
    #[error(
        "could not reach Redis at {endpoint} within {budget:?} ({attempts} attempt(s)): \
         check {url_var}, or widen the budget with {timeout_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
        timeout_var = ::nest_rs_config::var_name("redis", crate::config::CONNECT_TIMEOUT.key()),
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

/// A reply the queue's scripts and commands never send — a Redis that is not
/// the one the scripts ran on, or a defect here. Names the call and the shape it
/// expected, never what came back, which may carry a record.
#[derive(Debug, Error)]
#[error("Redis answered {call} in a shape it never sends: expected {expected}")]
pub(crate) struct UnexpectedReply {
    pub(crate) call: &'static str,
    pub(crate) expected: &'static str,
}

/// A checkpoint written by a delivery this worker no longer holds: its lease
/// lapsed and another delivery of the job holds it, so its state is that
/// delivery's to write.
#[derive(Debug, Error)]
#[error("the delivery writing this checkpoint no longer holds its job; another delivery does")]
pub(crate) struct CheckpointFenced;

/// A disposition the port added after this backend was written: the port
/// sends one only to a backend declaring the capability that names it, so
/// meeting one is a defect of the port or of this declaration.
#[derive(Debug, Error)]
#[error("the queue port ended a delivery in a way the Redis backend does not declare")]
pub(crate) struct UnknownDisposition;

/// A consumer prepared a second time: each worker builds its own, so a second
/// `prepare` is two workers sharing one consumer name in every group.
#[derive(Debug, Error)]
#[error("the Redis queue consumer was prepared twice; each queue worker binds its own")]
pub(crate) struct PreparedTwice;
