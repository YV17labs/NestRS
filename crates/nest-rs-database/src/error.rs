use std::error::Error;
use std::fmt;

use nest_rs_core::__private::SaidFailure;

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
pub enum OutboxError {
    /// The store's table is not there; `remedy` says how to create it.
    Missing {
        /// The table the store keeps its records in.
        table: &'static str,
        /// What creates it, as the driver's tooling spells it.
        remedy: &'static str,
    },
    /// The store's database refused a statement, or never answered it. The
    /// driver's error stays for its own `match`; every reader of the chain reads
    /// it as [`error_message`](fn@nest_rs_core::error_message) says it.
    Database(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for OutboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { table, remedy } => {
                write!(f, "the outbox table `{table}` is missing: {remedy}")
            }
            Self::Database(_) => f.write_str("the outbox store's database failed"),
        }
    }
}

impl fmt::Debug for OutboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { table, remedy } => f
                .debug_struct("Missing")
                .field("table", table)
                .field("remedy", remedy)
                .finish(),
            Self::Database(source) => f
                .debug_tuple("Database")
                .field(SaidFailure::of(source))
                .finish(),
        }
    }
}

// By hand: a derived `source()` hands out the driver's error, whose `Display` and
// `Debug` quote the row the database refused.
impl Error for OutboxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Missing { .. } => None,
            Self::Database(source) => Some(SaidFailure::of(source)),
        }
    }
}

/// Why an atomic block could not keep its promise.
#[non_exhaustive]
pub enum AtomicError {
    /// No ambient executor, or one whose driver offers no atomic block: nothing
    /// ran.
    Unsupported,
    /// The database refused to settle the block — its `RELEASE` or
    /// `ROLLBACK TO`, or the `COMMIT` of a block that is a transaction of its
    /// own — and nothing it wrote stands. Inside a unit of work, that unit is
    /// left unable to commit. A refused `SAVEPOINT` is not this: the block's
    /// first statement fails with the database's error. The driver's error stays
    /// for its own `match`, read as [`OutboxError::Database`]'s is.
    Database(Box<dyn std::error::Error + Send + Sync>),
    /// The work returned `Ok` over a statement that failed inside it: nothing
    /// it wrote stands.
    Poisoned {
        /// Whether the failed statement failed on something a fresh attempt
        /// could clear, read from the database's error.
        retryable: bool,
    },
    /// A handle to the block's executor outlived it: nothing it wrote stands.
    Escaped,
}

impl fmt::Display for AtomicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "no ambient executor offers an atomic block",
            Self::Database(_) => "the atomic block could not be settled",
            Self::Poisoned { .. } => {
                "a statement failed inside the atomic block; nothing it wrote stands"
            }
            Self::Escaped => "the atomic block's executor outlived it",
        })
    }
}

impl fmt::Debug for AtomicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("Unsupported"),
            Self::Database(source) => f
                .debug_tuple("Database")
                .field(SaidFailure::of(source))
                .finish(),
            Self::Poisoned { retryable } => f
                .debug_struct("Poisoned")
                .field("retryable", retryable)
                .finish(),
            Self::Escaped => f.write_str("Escaped"),
        }
    }
}

// By hand, as `OutboxError`'s.
impl Error for AtomicError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(source) => Some(SaidFailure::of(source)),
            Self::Unsupported | Self::Poisoned { .. } | Self::Escaped => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;

    use super::*;

    /// A driver's error as Postgres words a refusal: its message and its
    /// detail quote the row it refused.
    #[derive(Debug)]
    struct Refused {
        detail: &'static str,
    }

    impl fmt::Display for Refused {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "duplicate key value: {}", self.detail)
        }
    }

    impl Error for Refused {}

    fn refused() -> Box<dyn Error + Send + Sync> {
        Box::new(Refused {
            detail: "Key (email)=(zzqx@example.com) already exists.",
        })
    }

    /// A `{:?}` and a `main` returning the failure read the database's error
    /// as `error_message` says it — a driver's sayer claims its own — and never
    /// its `Debug`, which writes every field the server reported.
    #[test]
    fn no_reader_of_a_database_failure_prints_more_than_its_line_says() {
        fn read_by_all(err: impl Error + Send + Sync + 'static, sentence: &str) {
            assert_eq!(nest_rs_core::error_message(&err), sentence);
            let printed = nest_rs_testing::printed(&err);
            assert!(!printed.contains("Refused {"), "{printed}");
            assert!(printed.contains("duplicate key value"), "{printed}");
        }
        read_by_all(
            AtomicError::Database(refused()),
            "the atomic block could not be settled: duplicate key value: Key (email)=(zzqx@example.com) \
             already exists.",
        );
        read_by_all(
            OutboxError::Database(refused()),
            "the outbox store's database failed: duplicate key value: Key (email)=(zzqx@example.com) \
             already exists.",
        );
    }
}
