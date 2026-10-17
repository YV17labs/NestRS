use std::error::Error;

/// What an `#[on_event]` method answers: nothing, or a `Result` whose error
/// boxes — whatever the type is called.
#[diagnostic::on_unimplemented(
    message = "an `#[on_event]` method returns `()` or `Result<(), E>`, and `{Self}` is neither",
    label = "not `()`, nor a `Result<(), E>` whose `E` converts into `Box<dyn Error + Send + Sync>`",
    note = "an `Err` rolls the listener's unit of work back and is reported at `error` on \
            `nest_rs::events`; the next listener runs either way"
)]
pub trait ListenerAnswer {
    /// The answer, with its error boxed.
    fn into_outcome(self) -> Result<(), Box<dyn Error + Send + Sync>>;
}

impl ListenerAnswer for () {
    fn into_outcome(self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

impl<E: Into<Box<dyn Error + Send + Sync>>> ListenerAnswer for Result<(), E> {
    fn into_outcome(self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.map_err(Into::into)
    }
}

/// A listener's answer as the bus reads it — see [`ListenerAnswer`].
pub fn answered<A: ListenerAnswer>(answer: A) -> Result<(), Box<dyn Error + Send + Sync>> {
    answer.into_outcome()
}
