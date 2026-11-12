//! [`DocumentedMedia`] — the media type a handler's return type writes when no
//! schema names its body, said by its type for the route's document.
//!
//! `#[routes]` hands a handler's return type to [`MediaProbe`], and the compiler
//! answers which media type the document files the body under — bare, inside a
//! [`Created`], a [`Paginated`] or a `(StatusCode, B)` pair, inside a `Result` —
//! as [`PayloadProbe`](crate::__private::PayloadProbe) answers for a JSON body,
//! so an alias documents it.

use std::marker::PhantomData;

use bytes::Bytes;
use http::StatusCode;

use crate::body::Body;
use crate::created::Created;
use crate::paginated::Paginated;

/// A return type writing a body no schema names: text, bytes, a stream.
pub trait DocumentedMedia {
    /// The `Content-Type` it writes, without its parameters; `*/*` when it sets
    /// none.
    const MEDIA_TYPE: &'static str;
}

impl DocumentedMedia for String {
    const MEDIA_TYPE: &'static str = "text/plain";
}

impl DocumentedMedia for &'static str {
    const MEDIA_TYPE: &'static str = "text/plain";
}

impl DocumentedMedia for Bytes {
    const MEDIA_TYPE: &'static str = "application/octet-stream";
}

impl DocumentedMedia for Vec<u8> {
    const MEDIA_TYPE: &'static str = "application/octet-stream";
}

impl DocumentedMedia for Body {
    const MEDIA_TYPE: &'static str = "*/*";
}

impl<B: DocumentedMedia> DocumentedMedia for Created<B> {
    const MEDIA_TYPE: &'static str = B::MEDIA_TYPE;
}

impl<B: DocumentedMedia> DocumentedMedia for Paginated<B> {
    const MEDIA_TYPE: &'static str = B::MEDIA_TYPE;
}

impl<B: DocumentedMedia> DocumentedMedia for (StatusCode, B) {
    const MEDIA_TYPE: &'static str = B::MEDIA_TYPE;
}

impl<R: DocumentedMedia, E> DocumentedMedia for Result<R, E> {
    const MEDIA_TYPE: &'static str = R::MEDIA_TYPE;
}

/// Asks the compiler whether `R` is a [`DocumentedMedia`]: the inherent arm wins
/// over [`UndocumentedMediaProbe`]'s at the same autoref step whenever its bound holds.
pub struct MediaProbe<R>(PhantomData<fn() -> R>);

impl<R> Default for MediaProbe<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> MediaProbe<R> {
    /// The probe value the selection expression takes a reference to.
    pub const fn new() -> Self {
        MediaProbe(PhantomData)
    }
}

impl<R: DocumentedMedia> MediaProbe<R> {
    /// The documented arm.
    pub fn media_type(&self) -> Option<&'static str> {
        Some(R::MEDIA_TYPE)
    }
}

/// The fallback arm: every return type that names no media type — a JSON body,
/// which its schema says, a built `Response`, no body at all.
pub trait UndocumentedMediaProbe {
    /// No media type for the document.
    fn media_type(&self) -> Option<&'static str>;
}

impl<R> UndocumentedMediaProbe for &MediaProbe<R> {
    fn media_type(&self) -> Option<&'static str> {
        None
    }
}

/// The media type one return type writes, or `None`. The only spelling of the
/// selection: the extra `&` orders the two [`MediaProbe`] arms, as in
/// [`__payload_of`](crate::__payload_of).
#[doc(hidden)]
#[macro_export]
macro_rules! __media_of {
    ($ty:ty) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::UndocumentedMediaProbe as _;
        #[allow(
            clippy::needless_borrow,
            reason = "the extra borrow is what orders the two probe arms"
        )]
        let __nestrs_probe = &$crate::__private::MediaProbe::<$ty>::new();
        __nestrs_probe.media_type()
    }};
}

#[cfg(test)]
mod tests {
    use crate::{HttpError, Json, Response};

    use super::*;

    type Queued = (StatusCode, Bytes);

    #[test]
    fn a_body_no_schema_names_states_its_media_type_bare_wrapped_or_aliased() {
        assert_eq!(crate::__media_of!(String), Some("text/plain"));
        assert_eq!(crate::__media_of!(&'static str), Some("text/plain"));
        assert_eq!(
            crate::__media_of!(Result<String, HttpError>),
            Some("text/plain"),
        );
        assert_eq!(
            crate::__media_of!(Vec<u8>),
            Some("application/octet-stream")
        );
        assert_eq!(crate::__media_of!(Body), Some("*/*"));
        assert_eq!(
            crate::__media_of!(crate::Result<Queued>),
            Some("application/octet-stream"),
        );
        assert_eq!(
            crate::__media_of!(Created<Bytes>),
            Some("application/octet-stream"),
        );
        assert_eq!(crate::__media_of!(Paginated<String>), Some("text/plain"));
        assert_eq!(
            crate::__media_of!((StatusCode, Created<String>)),
            Some("text/plain"),
        );
    }

    #[test]
    fn a_body_a_schema_names_or_no_type_knows_states_none() {
        assert_eq!(crate::__media_of!(Json<u8>), None);
        assert_eq!(crate::__media_of!(Created<Json<u8>>), None);
        assert_eq!(crate::__media_of!((StatusCode, Json<u8>)), None);
        assert_eq!(crate::__media_of!(Response), None);
        assert_eq!(crate::__media_of!(crate::Result<Response>), None);
        assert_eq!(crate::__media_of!(StatusCode), None);
        assert_eq!(crate::__media_of!(()), None);
    }
}
