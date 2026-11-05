//! [`DocumentedBody`] — the request body an extractor reads and how it refuses
//! one, said by its type for the route's document.
//!
//! `#[routes]` reads a body off a `Json<T>` or `Form<T>` spelling; an extractor
//! that is the body itself names no wrapper, so the macro hands every handler
//! parameter's *type* to [`BodyProbe`], and the compiler answers whether that
//! type is a [`DocumentedBody`], as [`ShaperProbe`](crate::__private::ShaperProbe)
//! answers for a shaper.

use std::marker::PhantomData;

use crate::controller::{RequestBodyMeta, SchemaFn};

/// An extractor reading the request body as a type the route's document
/// names: the body `#[routes]` documents for a parameter of this type.
pub trait DocumentedBody {
    /// How the body arrives on the wire, and its content's schema.
    fn request_body() -> RequestBodyMeta;

    /// The schema of the `application/json` body a `400` answers, when the
    /// extractor refuses in its protocol's own words. The default, `None`, is the
    /// problem document the edge renders.
    fn refusal() -> Option<SchemaFn> {
        None
    }
}

/// Asks the compiler whether `T` is a [`DocumentedBody`].
///
/// Both arms below answer `request_body()` and `refusal()`; the inherent ones are
/// reachable only when their bound holds, and inherent methods win over trait
/// methods at the same autoref step. So [`__body_of`](crate::__body_of) answers
/// `Some` exactly when `T: DocumentedBody`, decided after name resolution.
pub struct BodyProbe<T>(PhantomData<fn() -> T>);

impl<T> Default for BodyProbe<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> BodyProbe<T> {
    /// The probe value the selection expression takes a reference to.
    pub const fn new() -> Self {
        BodyProbe(PhantomData)
    }
}

impl<T: DocumentedBody> BodyProbe<T> {
    /// The documented arm.
    pub fn request_body(&self) -> Option<RequestBodyMeta> {
        Some(T::request_body())
    }

    /// What the documented arm refuses in, when its own words.
    pub fn refusal(&self) -> Option<SchemaFn> {
        T::refusal()
    }
}

/// The fallback arm of the selection: every type that is not a
/// [`DocumentedBody`]. Reached by autoref, one step after the inherent
/// [`BodyProbe::request_body`].
pub trait UndocumentedProbe {
    /// No body for this parameter.
    fn request_body(&self) -> Option<RequestBodyMeta>;

    /// No refusal for this parameter.
    fn refusal(&self) -> Option<SchemaFn>;
}

impl<T> UndocumentedProbe for &BodyProbe<T> {
    fn request_body(&self) -> Option<RequestBodyMeta> {
        None
    }

    fn refusal(&self) -> Option<SchemaFn> {
        None
    }
}

/// The body one parameter type documents, or `None`. The only spelling of the
/// selection: the extra `&` is what orders the two [`BodyProbe`] arms, as in
/// [`__shaper_of`](crate::__shaper_of).
#[doc(hidden)]
#[macro_export]
macro_rules! __body_of {
    ($ty:ty) => {
        $crate::__body_probe!($ty, request_body)
    };
}

/// The refusal one parameter type documents, or `None`, selected as
/// [`__body_of`](crate::__body_of) selects its body.
#[doc(hidden)]
#[macro_export]
macro_rules! __refusal_of {
    ($ty:ty) => {
        $crate::__body_probe!($ty, refusal)
    };
}

/// `$answer` on the [`BodyProbe`] of `$ty`, the two arms ordered.
#[doc(hidden)]
#[macro_export]
macro_rules! __body_probe {
    ($ty:ty, $answer:ident) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::UndocumentedProbe as _;
        #[allow(
            clippy::needless_borrow,
            reason = "the extra borrow is what orders the two probe arms"
        )]
        let __nestrs_probe = &$crate::__private::BodyProbe::<$ty>::new();
        __nestrs_probe.$answer()
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Token;

    impl DocumentedBody for Token {
        fn request_body() -> RequestBodyMeta {
            RequestBodyMeta::Form(crate::controller::schema_of::<String>)
        }

        fn refusal() -> Option<SchemaFn> {
            Some(crate::controller::schema_of::<u32>)
        }
    }

    struct Plain;

    impl DocumentedBody for Plain {
        fn request_body() -> RequestBodyMeta {
            RequestBodyMeta::Form(crate::controller::schema_of::<String>)
        }
    }

    #[test]
    fn a_type_declaring_its_body_is_documented_and_any_other_is_not() {
        let documented = crate::__body_of!(Token).map(RequestBodyMeta::media_type);
        assert_eq!(documented, Some("application/x-www-form-urlencoded"));
        assert!(crate::__body_of!(String).is_none());
        assert!(crate::__body_of!(&crate::Request).is_none());
    }

    #[test]
    fn a_body_refuses_in_a_problem_document_unless_its_type_says_its_own_words() {
        assert!(crate::__refusal_of!(Token).is_some());
        assert!(crate::__refusal_of!(Plain).is_none());
        assert!(crate::__refusal_of!(String).is_none());
    }
}
