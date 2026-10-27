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
    /// The answer, its error boxed with every link kept as the type it is
    /// ([`boxed_error`](nest_rs_core::boxed_error)), so a driver's own sayer
    /// still claims its link on the bus's line.
    fn into_outcome(self) -> Result<(), Box<dyn Error + Send + Sync>>;
}

impl ListenerAnswer for () {
    fn into_outcome(self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

impl<E: Into<Box<dyn Error + Send + Sync>> + 'static> ListenerAnswer for Result<(), E> {
    fn into_outcome(self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.map_err(nest_rs_core::boxed_error)
    }
}

/// A listener's answer as the bus reads it — see [`ListenerAnswer`].
pub fn answered<A: ListenerAnswer>(answer: A) -> Result<(), Box<dyn Error + Send + Sync>> {
    answer.into_outcome()
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    /// A driver's error quoting a value, said without it by the driver's
    /// [`SafeLink`](nest_rs_core::SafeLink).
    #[derive(Debug)]
    struct Quoting;

    impl fmt::Display for Quoting {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("row refused: ada@example.com")
        }
    }

    impl Error for Quoting {}

    nest_rs_core::inventory::submit! {
        nest_rs_core::SafeLink::new(|link| link.is::<Quoting>().then(|| "row refused".to_owned()))
    }

    /// An error a listener answers keeps its links' types, an `anyhow::Error`'s
    /// included, so the driver's sayer still claims its own and the value stays
    /// off the bus's line; anyhow's own conversion boxes a wrapper whose
    /// `source` skips them.
    #[test]
    fn a_listener_error_is_said_by_its_links_own_sayers() {
        let bare = answered(Err::<(), _>(Quoting)).unwrap_err();
        assert_eq!(nest_rs_core::error_message(&*bare), "row refused");

        let anyhow = answered(Err::<(), _>(anyhow::Error::new(Quoting))).unwrap_err();
        assert_eq!(nest_rs_core::error_message(&*anyhow), "row refused");
    }
}
