//! [`DocumentedAnswer`] — what a handler's return type answers beside its
//! body, said by its type for the route's document.
//!
//! `#[routes]` hands a handler's return type to [`AnswerProbe`], and the
//! compiler answers whether it states its status and its headers, bare or
//! inside a `Result`, as [`BodyProbe`](crate::__private::BodyProbe) answers for
//! a body — so an alias documents what it answers. A type that does not is
//! answered at run time: a `(StatusCode, B)` pair, a bare `StatusCode`, a built
//! `Response`, an answer of an application's own.

use std::marker::PhantomData;

use bytes::Bytes;
use http::StatusCode;

use crate::body::Body;
use crate::created::Created;
use crate::json::Json;
use crate::paginated::Paginated;
use crate::problem::ProblemDetails;

/// The status and the headers an answer type sends beside its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnswerDoc {
    /// The success status it answers, or `None` when only the handler knows it.
    pub status: Option<u16>,
    /// It sends a `Location`.
    pub location: bool,
    /// It sends a `Link` to the next page whenever one follows.
    pub next_link: bool,
    /// It answers no success: a problem document, under an error status only
    /// the handler knows.
    pub error: bool,
}

impl AnswerDoc {
    /// `200`, and no header of its own.
    pub const OK: Self = Self {
        status: Some(200),
        location: false,
        next_link: false,
        error: false,
    };

    /// A status only the handler knows, and no header the document can name.
    const COMPUTED: Self = Self {
        status: None,
        ..Self::OK
    };
}

/// A return type that states its status: the bodies that answer `200`,
/// [`Created`], [`Paginated`] and a `Result` holding one, here, and a framework
/// crate's own answer type — `nest-rs-oauth-client`'s redirect to a provider,
/// and `nest-rs-social`'s around it — whose `Result` the blanket impl covers.
pub trait DocumentedAnswer {
    /// What it answers beside its body: `200` and no header of its own, unless
    /// it says otherwise.
    const ANSWER: AnswerDoc = AnswerDoc::OK;
}

const CREATED: AnswerDoc = AnswerDoc {
    status: Some(201),
    location: true,
    ..AnswerDoc::OK
};

const PAGINATED: AnswerDoc = AnswerDoc {
    next_link: true,
    ..AnswerDoc::OK
};

impl<T> DocumentedAnswer for Created<T> {
    const ANSWER: AnswerDoc = CREATED;
}

impl<T> DocumentedAnswer for Paginated<T> {
    const ANSWER: AnswerDoc = PAGINATED;
}

/// A status the handler computes, over what the answer it wraps sends beside it.
impl<B: DocumentedAnswer> DocumentedAnswer for (StatusCode, B) {
    const ANSWER: AnswerDoc = AnswerDoc {
        status: None,
        ..B::ANSWER
    };
}

impl<T> DocumentedAnswer for Json<T> {}

impl DocumentedAnswer for String {}

impl DocumentedAnswer for &'static str {}

impl DocumentedAnswer for Bytes {}

impl DocumentedAnswer for Vec<u8> {}

impl DocumentedAnswer for Body {}

impl DocumentedAnswer for () {}

/// A problem document answered as a handler's success answers its error, under
/// a status only the handler knows.
impl DocumentedAnswer for ProblemDetails {
    const ANSWER: AnswerDoc = AnswerDoc {
        status: None,
        error: true,
        ..AnswerDoc::OK
    };
}

impl<R: DocumentedAnswer, E> DocumentedAnswer for Result<R, E> {
    const ANSWER: AnswerDoc = R::ANSWER;
}

/// Asks the compiler whether `R` is a [`DocumentedAnswer`]: the inherent arm
/// wins over [`ComputedAnswerProbe`]'s at the same autoref step whenever its
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
    pub fn answer(&self) -> AnswerDoc {
        R::ANSWER
    }
}

/// The fallback arm: every return type that does not state its status.
pub trait ComputedAnswerProbe {
    /// A status only the handler knows.
    fn answer(&self) -> AnswerDoc;
}

impl<R> ComputedAnswerProbe for &AnswerProbe<R> {
    fn answer(&self) -> AnswerDoc {
        AnswerDoc::COMPUTED
    }
}

/// What one return type answers beside its body. The only spelling of the
/// selection: the extra `&` orders the two [`AnswerProbe`] arms, as in
/// [`__body_of`](crate::__body_of).
#[doc(hidden)]
#[macro_export]
macro_rules! __answer_of {
    ($ty:ty) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::ComputedAnswerProbe as _;
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
        assert_eq!(crate::__answer_of!(Created<Json<u8>>), CREATED);
        assert_eq!(
            crate::__answer_of!(Result<Created<Json<u8>>, HttpError>),
            CREATED,
        );
        assert_eq!(crate::__answer_of!(crate::Result<Minted>), CREATED);
        assert_eq!(
            crate::__answer_of!(crate::Result<Paginated<Json<Vec<u8>>>>),
            PAGINATED,
        );
    }

    #[test]
    fn a_problem_answered_as_the_success_is_an_error_only_the_handler_knows() {
        for answer in [
            crate::__answer_of!(ProblemDetails),
            crate::__answer_of!(crate::Result<ProblemDetails>),
        ] {
            assert_eq!((answer.status, answer.error), (None, true));
        }
        assert!(!crate::__answer_of!((StatusCode, Json<u8>)).error);
    }

    #[test]
    fn a_body_alone_answers_200_with_no_header_of_its_own() {
        assert_eq!(crate::__answer_of!(Json<u8>), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(String), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(&'static str), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(Bytes), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(Vec<u8>), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(crate::Body), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(()), AnswerDoc::OK);
        assert_eq!(crate::__answer_of!(crate::Result<Json<u8>>), AnswerDoc::OK);
    }

    #[test]
    fn a_status_computed_over_a_created_or_a_paginated_keeps_the_header_it_sends() {
        let computed = crate::__answer_of!((StatusCode, Created<Json<u8>>));
        assert_eq!((computed.status, computed.location), (None, true));
        let computed = crate::__answer_of!(crate::Result<(StatusCode, Paginated<Json<u8>>)>);
        assert_eq!((computed.status, computed.next_link), (None, true));
    }

    type Queued = (StatusCode, Json<u8>);

    struct OwnAnswer;

    #[test]
    fn an_answer_that_does_not_state_its_status_leaves_it_to_the_handler() {
        assert_eq!(crate::__answer_of!(StatusCode), AnswerDoc::COMPUTED);
        assert_eq!(
            crate::__answer_of!(crate::Result<StatusCode>),
            AnswerDoc::COMPUTED
        );
        assert_eq!(
            crate::__answer_of!((StatusCode, Json<u8>)),
            AnswerDoc::COMPUTED
        );
        assert_eq!(
            crate::__answer_of!(crate::Result<Queued>),
            AnswerDoc::COMPUTED
        );
        assert_eq!(crate::__answer_of!(Response), AnswerDoc::COMPUTED);
        assert_eq!(
            crate::__answer_of!(crate::Result<Response>),
            AnswerDoc::COMPUTED
        );
        assert_eq!(crate::__answer_of!(OwnAnswer), AnswerDoc::COMPUTED);
        assert_eq!(
            crate::__answer_of!((StatusCode, OwnAnswer)),
            AnswerDoc::COMPUTED
        );
    }
}
