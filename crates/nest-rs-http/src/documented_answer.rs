//! [`DocumentedAnswer`] — what a handler's return type answers beside its
//! body, said by its type for the route's document.
//!
//! `#[routes]` hands a handler's return type to [`AnswerProbe`], and the
//! compiler answers whether it is a [`Created`] or a [`Paginated`] answer,
//! bare or inside a `Result`, as [`BodyProbe`](crate::__private::BodyProbe)
//! answers for a body — so an alias documents what it answers.

use std::marker::PhantomData;

use crate::created::Created;
use crate::paginated::Paginated;

/// The status and the headers an answer type sends beside its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnswerDoc {
    /// The success status it answers.
    pub status: u16,
    /// It sends a `Location`.
    pub location: bool,
    /// It sends a `Link` to the next page whenever one follows.
    pub next_link: bool,
}

/// A return type that answers a status or a header of its own: [`Created`],
/// [`Paginated`] and a `Result` holding one, here, and a framework crate's
/// own answer type — `nest-rs-oauth-client`'s redirect to a provider, and
/// `nest-rs-social`'s around it — whose `Result` the blanket impl covers.
pub trait DocumentedAnswer {
    /// What it answers beside its body.
    const ANSWER: AnswerDoc;
}

const CREATED: AnswerDoc = AnswerDoc {
    status: 201,
    location: true,
    next_link: false,
};

const PAGINATED: AnswerDoc = AnswerDoc {
    status: 200,
    location: false,
    next_link: true,
};

impl<T> DocumentedAnswer for Created<T> {
    const ANSWER: AnswerDoc = CREATED;
}

impl<T> DocumentedAnswer for Paginated<T> {
    const ANSWER: AnswerDoc = PAGINATED;
}

impl<R: DocumentedAnswer, E> DocumentedAnswer for Result<R, E> {
    const ANSWER: AnswerDoc = R::ANSWER;
}

/// Asks the compiler whether `R` is a [`DocumentedAnswer`]: the inherent arm
/// wins over [`PlainAnswerProbe`]'s at the same autoref step whenever its
/// bound holds.
pub struct AnswerProbe<R>(PhantomData<fn() -> R>);

impl<R> Default for AnswerProbe<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> AnswerProbe<R> {
    /// The probe value the selection expression takes a reference to.
    pub const fn new() -> Self {
        AnswerProbe(PhantomData)
    }
}

impl<R: DocumentedAnswer> AnswerProbe<R> {
    /// The documented arm.
    pub fn answer(&self) -> Option<AnswerDoc> {
        Some(R::ANSWER)
    }
}

/// The fallback arm: every return type that answers nothing beside its body.
pub trait PlainAnswerProbe {
    /// Nothing beyond the body.
    fn answer(&self) -> Option<AnswerDoc>;
}

impl<R> PlainAnswerProbe for &AnswerProbe<R> {
    fn answer(&self) -> Option<AnswerDoc> {
        None
    }
}

/// What one return type answers beside its body, or `None`. The only
/// spelling of the selection: the extra `&` orders the two [`AnswerProbe`]
/// arms, as in [`__body_of`](crate::__body_of).
#[doc(hidden)]
#[macro_export]
macro_rules! __answer_of {
    ($ty:ty) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::PlainAnswerProbe as _;
        #[allow(
            clippy::needless_borrow,
            reason = "the extra borrow is what orders the two probe arms"
        )]
        let __nestrs_probe = &$crate::__private::AnswerProbe::<$ty>::new();
        __nestrs_probe.answer()
    }};
}

#[cfg(test)]
mod tests {
    use crate::{HttpError, Json, Response};

    use super::*;

    type Minted = Created<Json<u8>>;

    #[test]
    fn a_created_or_paginated_answer_is_documented_bare_inside_a_result_or_aliased() {
        assert_eq!(crate::__answer_of!(Created<Json<u8>>), Some(CREATED));
        assert_eq!(
            crate::__answer_of!(Result<Created<Json<u8>>, HttpError>),
            Some(CREATED),
        );
        assert_eq!(crate::__answer_of!(crate::Result<Minted>), Some(CREATED));
        assert_eq!(
            crate::__answer_of!(crate::Result<Paginated<Json<Vec<u8>>>>),
            Some(PAGINATED),
        );
    }

    #[test]
    fn any_other_answer_says_nothing_beyond_its_body() {
        assert_eq!(crate::__answer_of!(Json<u8>), None);
        assert_eq!(crate::__answer_of!(crate::Result<Response>), None);
        assert_eq!(crate::__answer_of!(()), None);
    }
}
