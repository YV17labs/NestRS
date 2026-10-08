use crate::PipeError;

/// A pipe `transform`s an extracted value into a new value or a [`PipeError`].
///
/// Pipes are stateless zero-sized markers named at a call site
/// (`Piped<ParseInt, _>`), so `transform` is an associated function.
pub trait Pipe {
    /// The value the pipe receives (the extractor's output).
    type In;
    /// The value the pipe hands the handler.
    type Out;
    /// Convert `input`, or reject it with a [`PipeError`].
    fn transform(input: Self::In) -> Result<Self::Out, PipeError>;
}
