/// What [`Repo::insert_unscoped_or_existing`](crate::Repo::insert_unscoped_or_existing)
/// found: the row it inserted, or the live row already holding its key.
///
/// Exhaustive on purpose: an insert either landed or met the row holding its
/// key, so a caller matches both and never needs a `_` arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "closed by definition: an insert lands, or meets the row holding its key"
)]
pub enum Insertion<M> {
    /// The row was inserted, and is returned as the database stored it.
    New(M),
    /// A live row already held the key: nothing was written, and that row is
    /// returned.
    Existing(M),
}
