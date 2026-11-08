//! [`RowProbe`]: whether a type a dispatched method answers is a row, asked of the
//! compiler where the method is declared, so a `Model` no `#[expose]` decorates
//! crosses no wire whatever it derives and whatever holds it.
//!
//! An edge's impl half writes one call per type its method's answer spells
//! (`Json<Vec<secret::Model>>` asks of `Json<…>`, `Vec<…>` and `secret::Model`):
//! the inherent arm, which method resolution prefers, exists only for a
//! `sea_orm::ModelTrait`, and its bound names the fix; [`RowProbeFallback`]
//! answers for every other type. A build without `sea-orm` holds no row, and the
//! fallback answers alone.

use std::marker::PhantomData;

/// One type a dispatched method answers, for the compiler to be asked whether it
/// is a row.
pub struct RowProbe<T: ?Sized>(PhantomData<fn() -> *const T>);

impl<T: ?Sized> RowProbe<T> {
    /// The probe the expansion asks.
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: ?Sized> Default for RowProbe<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "seaorm")]
impl<T: sea_orm::ModelTrait> RowProbe<T> {
    /// A row: refused, by a bound no type meets.
    pub fn answered(&self)
    where
        T: CrossesAWire,
    {
    }
}

/// What every type that is no row answers.
pub trait RowProbeFallback {
    /// Not a row: nothing to refuse.
    fn answered(&self) {}
}

impl<T: ?Sized> RowProbeFallback for RowProbe<T> {}

/// What a type a dispatched method answers must be when it is a `Model`, and
/// none is: a row holds every column of its table.
#[diagnostic::on_unimplemented(
    message = "a row crosses no wire: `{Self}` holds every column of its table, the ones no reply answers included",
    label = "this answers a `Model`",
    note = "answer the type `#[expose]` builds from the row (`From<&Model>`), or a DTO naming the columns it answers: a derive on a `Model` opens no wire"
)]
pub trait CrossesAWire {}

#[cfg(test)]
mod tests {
    use super::{RowProbe, RowProbeFallback as _};

    #[test]
    fn a_type_that_is_no_row_answers_through_the_fallback() {
        RowProbe::<Vec<String>>::new().answered();
        RowProbe::<str>::new().answered();
    }
}
