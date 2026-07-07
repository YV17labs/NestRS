use std::marker::PhantomData;

use validator::Validate;

use crate::pipe::{Pipe, PipeError};

/// Validate a value with `validator::Validate`, returning it unchanged on
/// success and a field-level [`PipeError`] (the `validator` errors as `details`)
/// on failure. The HTTP transport exposes this as `Valid<Json<T>>`.
pub struct ValidationPipe<T>(PhantomData<fn() -> T>);

impl<T: Validate> Pipe for ValidationPipe<T> {
    type In = T;
    type Out = T;
    fn transform(input: T) -> Result<T, PipeError> {
        match input.validate() {
            Ok(()) => Ok(input),
            // Shared with the global `ValidateProbe` path so the submitted
            // value (`params.value`) is stripped from the details on every
            // transport — a rejected credential never echoes back.
            Err(errors) => Err(crate::validate::validation_error(errors)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    #[derive(Debug, Validate)]
    struct Signup {
        #[validate(email)]
        email: String,
    }

    #[test]
    fn passes_valid_and_rejects_invalid_with_details() {
        let ok = Signup {
            email: "a@b.io".into(),
        };
        assert!(ValidationPipe::<Signup>::transform(ok).is_ok());

        let bad = Signup {
            email: "nope".into(),
        };
        let err = ValidationPipe::<Signup>::transform(bad).unwrap_err();
        assert!(err.details().is_some());
    }

    #[test]
    fn rejection_details_never_echo_the_submitted_value() {
        // A too-short secret must not come back in the details — only the field
        // name and the constraint bound survive, so the message stays
        // actionable ("password: length min 8") without leaking what was typed.
        #[derive(Debug, Validate)]
        struct Login {
            #[validate(length(min = 8))]
            password: String,
        }
        let err = ValidationPipe::<Login>::transform(Login {
            password: "hunter2".into(), // 7 chars — fails `min = 8`
        })
        .unwrap_err();
        let details = err.details().expect("field details present");
        let text = details.to_string();
        assert!(
            !text.contains("hunter2"),
            "submitted value must not be echoed: {text}",
        );
        assert!(
            details.get("password").is_some(),
            "the failing field name is kept: {text}",
        );
        assert!(text.contains('8'), "the constraint bound is kept: {text}");
    }
}
