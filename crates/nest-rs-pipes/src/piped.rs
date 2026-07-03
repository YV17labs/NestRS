//! Value-form pipe carriers for transports whose handler argument *is* the wire
//! value (GraphQL, WS, queue) — the analog of the HTTP `Piped<P, E>` / `Valid<E>`
//! extractors, which wrap a poem *extractor* `E`.
//!
//! HTTP can wrap a `FromRequest` extractor because poem calls it per argument.
//! The other transports deserialize a single typed value, so there is no
//! extractor to wrap: each transport's macro (`#[resolver]`, `#[messages]`,
//! `#[processor]`) strips `Piped<P, T>` / `Valid<T>` from the *wire* signature
//! (exposing `T`), runs the pipe, and hands the handler this carrier. Same
//! developer surface as HTTP (`into_inner` / `Deref`), a different binding —
//! the framework already splits one concept across transports this way
//! (`Bind<S, A>` on HTTP vs `bind` on GraphQL).

use std::marker::PhantomData;
use std::ops::Deref;

use validator::Validate;

use crate::{Pipe, PipeError, ValidationPipe};

/// Applies pipe `P` to a handler argument whose wire type is `T` (`T` is
/// `P::In`). The transport macro exposes `T` on the wire, calls [`Piped::apply`]
/// with the extracted value, and hands the handler the transformed `P::Out`.
pub struct Piped<P: Pipe, T> {
    value: P::Out,
    _marker: PhantomData<fn() -> (P, T)>,
}

impl<P: Pipe<In = T>, T> Piped<P, T> {
    /// Run `P` over an extracted wire value. Called by a transport's macro after
    /// it deserializes `T`; the macro surfaces a [`PipeError`] as that
    /// transport's native error.
    pub fn apply(input: T) -> Result<Self, PipeError> {
        Ok(Self {
            value: P::transform(input)?,
            _marker: PhantomData,
        })
    }
}

impl<P: Pipe, T> Piped<P, T> {
    pub fn into_inner(self) -> P::Out {
        self.value
    }
}

impl<P: Pipe, T> Deref for Piped<P, T> {
    type Target = P::Out;
    fn deref(&self) -> &P::Out {
        &self.value
    }
}

/// Validates a handler argument of wire type `T` with `validator::Validate` —
/// the value-form of the HTTP `Valid<E>`. `Valid<T>` is the ergonomic form of
/// `Piped<ValidationPipe<T>, T>`; the transport macro exposes `T` on the wire
/// and calls [`Valid::apply`].
pub struct Valid<T>(T);

impl<T: Validate> Valid<T> {
    pub fn apply(input: T) -> Result<Self, PipeError> {
        Ok(Valid(ValidationPipe::<T>::transform(input)?))
    }
}

impl<T> Valid<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Valid<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ToUpper;

    impl Pipe for ToUpper {
        type In = String;
        type Out = String;
        fn transform(input: String) -> Result<String, PipeError> {
            Ok(input.to_ascii_uppercase())
        }
    }

    struct AlwaysReject;

    impl Pipe for AlwaysReject {
        type In = String;
        type Out = String;
        fn transform(_: String) -> Result<String, PipeError> {
            Err(PipeError::new("nope"))
        }
    }

    #[test]
    fn apply_transforms_then_into_inner_yields_the_output() {
        let piped = Piped::<ToUpper, String>::apply("hello".into()).expect("happy path");
        assert_eq!(piped.into_inner(), "HELLO");
    }

    #[test]
    fn deref_borrows_the_transformed_value() {
        let piped = Piped::<ToUpper, String>::apply("world".into()).expect("happy path");
        assert_eq!(piped.len(), 5);
        assert_eq!(&*piped, "WORLD");
    }

    #[test]
    fn apply_surfaces_the_pipe_error() {
        let Err(err) = Piped::<AlwaysReject, String>::apply("x".into()) else {
            panic!("the pipe should have rejected");
        };
        assert_eq!(err.message(), "nope");
    }

    #[derive(Validate)]
    struct Greeting {
        #[validate(length(min = 1))]
        msg: String,
    }

    #[test]
    fn valid_apply_passes_a_valid_value() {
        let v = Valid::apply(Greeting { msg: "hi".into() }).expect("valid");
        assert_eq!(v.into_inner().msg, "hi");
    }

    #[test]
    fn valid_apply_rejects_an_invalid_value_with_field_details() {
        let Err(err) = Valid::apply(Greeting { msg: String::new() }) else {
            panic!("validation should have rejected");
        };
        assert_eq!(err.message(), "validation failed");
        assert!(err.details().is_some());
    }
}
