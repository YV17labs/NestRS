//! [`DocumentedPayload`] — the body a handler's return type answers, said by
//! its type for the route's document.
//!
//! `#[routes]` hands a handler's return type to [`PayloadProbe`], and the
//! compiler answers which type the body's schema is read off — a `Json<T>`'s
//! `T`, bare, inside a [`Created`], a [`Paginated`] or a `(StatusCode, B)` pair,
//! inside a `Result` — as [`AnswerProbe`](crate::__private::AnswerProbe) answers
//! for the status, so an alias documents its body.

use std::marker::PhantomData;

use http::StatusCode;
use schemars::JsonSchema;

use crate::controller::{SchemaFn, schema_of};
use crate::created::Created;
use crate::json::Json;
use crate::paginated::Paginated;

/// A return type whose body is JSON of a type the route's document names.
///
/// Implemented here for [`Json`], a [`Created`], [`Paginated`] or
/// `(StatusCode, B)` answer of a documented body, and a `Result` of one; a
/// sibling crate implements it for a typed answer that renders its own JSON.
pub trait DocumentedPayload {
    /// The type whose JSON schema the body follows.
    type Payload;
}

impl<T> DocumentedPayload for Json<T> {
    type Payload = T;
}

impl<B: DocumentedPayload> DocumentedPayload for Created<B> {
    type Payload = B::Payload;
}

impl<B: DocumentedPayload> DocumentedPayload for Paginated<B> {
    type Payload = B::Payload;
}

impl<B: DocumentedPayload> DocumentedPayload for (StatusCode, B) {
    type Payload = B::Payload;
}

impl<R: DocumentedPayload, E> DocumentedPayload for Result<R, E> {
    type Payload = R::Payload;
}

/// Asks the compiler whether `R` is a [`DocumentedPayload`]: the inherent arm
/// wins over [`UndocumentedPayloadProbe`]'s at the same autoref step whenever
/// its impl's bound holds.
pub struct PayloadProbe<R>(PhantomData<fn() -> R>);

impl<R> Default for PayloadProbe<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> PayloadProbe<R> {
    /// The probe value the selection expression takes a reference to.
    pub const fn new() -> Self {
        PayloadProbe(PhantomData)
    }
}

impl<R: DocumentedPayload> PayloadProbe<R> {
    /// The documented arm. Its schema bound sits on the method, which the
    /// selection never reads, so a JSON answer whose payload has no schema is
    /// a compile error rather than a body the document drops.
    pub fn payload(&self) -> Option<SchemaFn>
    where
        R::Payload: JsonSchema,
    {
        Some(schema_of::<R::Payload>)
    }
}

/// The fallback arm: every return type whose body no type names — a built
/// `Response`, a string, a stream.
pub trait UndocumentedPayloadProbe {
    /// No body for the document.
    fn payload(&self) -> Option<SchemaFn>;
}

impl<R> UndocumentedPayloadProbe for &PayloadProbe<R> {
    fn payload(&self) -> Option<SchemaFn> {
        None
    }
}

/// The schema of the body one return type answers, or `None`. The only
/// spelling of the selection: the extra `&` orders the two [`PayloadProbe`]
/// arms, as in [`__answer_of`](crate::__answer_of).
#[doc(hidden)]
#[macro_export]
macro_rules! __payload_of {
    ($ty:ty) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::UndocumentedPayloadProbe as _;
        #[allow(
            clippy::needless_borrow,
            reason = "the extra borrow is what orders the two probe arms"
        )]
        let __nestrs_probe = &$crate::__private::PayloadProbe::<$ty>::new();
        __nestrs_probe.payload()
    }};
}

#[cfg(test)]
mod tests {
    use crate::{HttpError, Response};

    use super::*;

    type Minted = Created<Json<String>>;

    /// The schema a probed body is documented with, rendered.
    fn documented(payload: Option<SchemaFn>) -> Option<serde_json::Value> {
        payload.map(|schema_of| schema_of(&mut schemars::SchemaGenerator::default()).to_value())
    }

    #[test]
    fn a_json_body_is_documented_bare_inside_a_typed_answer_inside_a_result_or_aliased() {
        let string = Some(serde_json::json!({ "type": "string" }));
        assert_eq!(documented(crate::__payload_of!(Json<String>)), string);
        assert_eq!(documented(crate::__payload_of!(Minted)), string);
        assert_eq!(
            documented(crate::__payload_of!(
                Result<Created<Json<String>>, HttpError>
            )),
            string,
        );
        assert_eq!(
            documented(crate::__payload_of!(crate::Result<Minted>)),
            string
        );
        assert_eq!(
            documented(crate::__payload_of!(crate::Result<Paginated<Json<String>>>)),
            string,
        );
        assert_eq!(
            documented(crate::__payload_of!(Json<&'static str>)),
            string,
            "a borrowed payload names its schema as its owner would",
        );
        assert_eq!(
            documented(crate::__payload_of!(Json<std::borrow::Cow<'_, str>>)),
            string,
            "a lifetime the expression infers names the same schema",
        );
    }

    #[test]
    fn a_body_no_type_names_is_left_out() {
        assert!(crate::__payload_of!(Response).is_none());
        assert!(crate::__payload_of!(crate::Result<Response>).is_none());
        assert!(crate::__payload_of!(String).is_none());
        assert!(crate::__payload_of!(()).is_none());
        assert!(crate::__payload_of!(Created<String>).is_none());
    }
}
