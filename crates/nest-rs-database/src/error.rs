/// A successful unit its driver could not honour: what went wrong, and whether
/// running the same unit again could end differently.
///
/// A driver answers from what the database reported, never from a guess: a
/// serialization failure or a deadlock is retryable; a constraint violation is
/// not, nor a commit whose outcome is *unknown*, which may have landed.
///
/// It can grow within 7.x: read its fields, and build one through a
/// constructor, never a literal:
///
/// ```compile_fail,E0639
/// let why = nest_rs_database::Unhonoured {
///     reason: "the unit's transaction could not be committed",
///     retryable: false,
/// };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Unhonoured {
    /// What the driver could not do, as the sentence the edge reports; the
    /// database's own error is logged by the driver that saw it.
    pub reason: &'static str,
    /// Whether re-running the unit could produce a different outcome.
    pub retryable: bool,
}

impl Unhonoured {
    /// A unit whose commit did not answer within its driver's settle net
    /// ([`UnitOfWork::settle_net`](crate::UnitOfWork::settle_net)): its writes
    /// may have landed, so running it again could write them twice — never
    /// retried, as a two-phase commit leaves an *in-doubt* transaction to an
    /// operator.
    pub const IN_DOUBT: Self = Self::deterministic(
        "the unit's commit did not answer within its bound; its writes may have landed",
    );

    /// A failure whose retryability was read off what the backend said.
    pub const fn new(reason: &'static str, retryable: bool) -> Self {
        Self { reason, retryable }
    }

    /// A failure a retry could clear.
    pub const fn retryable(reason: &'static str) -> Self {
        Self::new(reason, true)
    }

    /// A failure a retry would only repeat.
    pub const fn deterministic(reason: &'static str) -> Self {
        Self::new(reason, false)
    }
}

impl std::fmt::Display for Unhonoured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for Unhonoured {}

/// Why the outbox could not keep a record, or hand one over.
///
/// It can grow within 7.x, so a `match` outside this crate keeps a `_` arm.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum OutboxError {
    /// The store's table is not there; `remedy` says how to create it.
    #[error("the outbox table `{table}` is missing: {remedy}")]
    Missing {
        /// The table the store keeps its records in.
        table: &'static str,
        /// What creates it, as the driver's tooling spells it.
        remedy: &'static str,
    },
    /// The store's database refused a statement, or never answered it; the
    /// source says why, by kind, without a value.
    #[error("the outbox store's database failed")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Why an atomic block could not keep its promise.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AtomicError {
    /// No ambient executor, or one whose driver offers no atomic block: nothing
    /// ran.
    #[error("no ambient executor offers an atomic block")]
    Unsupported,
    /// The database refused to settle the block — its `RELEASE` or
    /// `ROLLBACK TO`, or the `COMMIT` of a block that is a transaction of its
    /// own — and nothing it wrote stands. Inside a unit of work the framework
    /// opened, that unit is left unable to commit; inside a transaction the
    /// caller opened, the database has aborted it and the caller rolls it back.
    /// A refused `SAVEPOINT` is not this: the block's first statement fails
    /// with the database's error.
    #[error("the atomic block could not be settled")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The work returned `Ok` over a statement that failed inside it: nothing
    /// it wrote stands.
    #[error("a statement failed inside the atomic block; nothing it wrote stands")]
    Poisoned {
        /// Whether the failed statement failed on something a fresh attempt
        /// could clear, read from the database's error.
        retryable: bool,
    },
    /// A handle to the block's executor outlived it: nothing it wrote stands.
    #[error("the atomic block's executor outlived it")]
    Escaped,
}
