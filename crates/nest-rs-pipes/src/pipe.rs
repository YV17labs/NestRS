use crate::PipeError;

/// A pipe `transform`s an extracted value into a new value or a [`PipeError`].
///
/// Pipes are **stateless** — a zero-sized marker named at a call site
/// (`Piped<ParseInt, _>`), never instantiated — so `transform` is an associated
/// function. Stateful/DI-injected pipes would need a different binding.
pub trait Pipe {
    /// The value the pipe receives (the extractor's output).
    type In;
    /// The value the pipe hands the handler.
    type Out;
    /// Convert `input`, or reject it with a [`PipeError`].
    fn transform(input: Self::In) -> Result<Self::Out, PipeError>;
}
