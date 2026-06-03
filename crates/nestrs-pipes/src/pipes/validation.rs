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
            Err(errors) => Err(PipeError::with_details(
                "validation failed",
                serde_json::to_value(errors).unwrap_or(serde_json::Value::Null),
            )),
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
}
