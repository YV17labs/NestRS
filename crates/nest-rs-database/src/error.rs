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
