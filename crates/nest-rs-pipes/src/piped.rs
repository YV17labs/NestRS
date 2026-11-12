//! The per-argument pipe carrier: one type on every edge, over the [`Source`]
//! its input is read from.
//!
//! An edge that decodes its argument itself (GraphQL, WebSocket, MCP, a job)
//! reads `Piped<P>`, whose wire type is the pipe's `In`; HTTP names the
//! extractor that reads it, `Piped<P, Json<T>>`. Either way `Piped(value)`
//! binds the pipe's output.

use std::ops::{Deref, DerefMut};

use crate::{Pipe, PipeError};

/// Where a [`Piped`] argument's input is read from: the value its edge decoded
/// ([`Decoded`], the default) or what an HTTP extractor carries (`Json<T>`,
/// `Query<T>`, `Form<T>`, `Path<T>`, `Header<T>`).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a source a pipe reads",
    note = "on GraphQL, WebSockets, MCP and queues write `Piped<P>`: the edge decodes the \
            pipe's `In` itself; on HTTP name the extractor that reads it, `Piped<P, Json<T>>`"
)]
pub trait Source {
    /// The value it hands the pipe.
    type Value;
    /// Hand that value over, by value.
    fn into_value(self) -> Self::Value;
}

/// The value an edge decodes itself, as a [`Piped`] argument's source: the
/// default, so `Piped<P>` reads `P::In` off the wire.
pub struct Decoded<T>(pub T);

impl<T> Source for Decoded<T> {
    type Value = T;
    fn into_value(self) -> T {
        self.0
    }
}

/// Pipe `Self` run over source `S`, whose value is the pipe's input: what a
/// [`Piped`] argument holds is its `Output`.
#[diagnostic::on_unimplemented(
    message = "pipe `{Self}` does not read what `{S}` carries",
    note = "a pipe's `In` is the value its source hands over"
)]
pub trait PipeOver<S>: Pipe {
    /// The pipe's output.
    type Output;
}

impl<P: Pipe, S: Source<Value = P::In>> PipeOver<S> for P {
    type Output = P::Out;
}

/// Runs pipe `P` over an argument read from `S`, and hands the handler the
/// pipe's output: `Piped(id): Piped<ParseUuid, Path<String>>` on HTTP,
/// `Piped(name): Piped<Trim>` where the edge decodes the argument itself.
///
/// The value the source hands over is held to the rules its type declares
/// before the pipe runs, as every decoded argument is; the output is the
/// pipe's own.
///
/// ```
/// use nest_rs_pipes::{Piped, Trim};
///
/// let Piped(name) = Piped::<Trim>::apply("  ada ".to_owned())?;
/// assert_eq!(name, "ada");
/// # Ok::<(), nest_rs_pipes::PipeError>(())
/// ```
pub struct Piped<P: PipeOver<S>, S = Decoded<<P as Pipe>::In>>(pub P::Output);

impl<P: Pipe, S: Source<Value = P::In>> Piped<P, S> {
    /// Run `P` over a value read from `S`. Called by an edge once it decoded
    /// the value; the edge renders a [`PipeError`] as its own error.
    pub fn apply(input: P::In) -> Result<Self, PipeError> {
        P::transform(input).map(Piped)
    }

    /// Take ownership of the pipe's output.
    pub fn into_inner(self) -> P::Out {
        self.0
    }
}

impl<P: PipeOver<S>, S> Deref for Piped<P, S> {
    type Target = P::Output;
    fn deref(&self) -> &P::Output {
        &self.0
    }
}

impl<P: PipeOver<S>, S> DerefMut for Piped<P, S> {
    fn deref_mut(&mut self) -> &mut P::Output {
        &mut self.0
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

    /// A source of its own, as an HTTP extractor is.
    struct Carried(String);

    impl Source for Carried {
        type Value = String;
        fn into_value(self) -> String {
            self.0
        }
    }

    #[test]
    fn apply_transforms_and_the_carrier_destructures_to_the_output() {
        let Piped(shouted) = Piped::<ToUpper>::apply("hello".into()).expect("happy path");
        assert_eq!(shouted, "HELLO");
    }

    #[test]
    fn a_source_names_where_the_input_was_read_and_changes_nothing_else() {
        let carried = Carried("world".into());
        let piped = Piped::<ToUpper, Carried>::apply(carried.into_value()).expect("happy path");
        assert_eq!(piped.len(), 5);
        assert_eq!(&*piped, "WORLD");
        assert_eq!(piped.into_inner(), "WORLD");
    }

    #[test]
    fn apply_surfaces_the_pipe_error() {
        let Err(err) = Piped::<AlwaysReject>::apply("x".into()) else {
            panic!("the pipe should have rejected");
        };
        assert_eq!(err.message(), "nope");
    }
}
