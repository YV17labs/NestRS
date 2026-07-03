//! Autoref-specialization probe for *global* validation — the analog of NestJS's
//! `app.useGlobalPipes(new ValidationPipe())`, minus the reflection Rust doesn't
//! have.
//!
//! A transport macro can't tell at codegen time whether a handler's input type
//! carries `validator::Validate` rules. This probe lets it emit **one uniform
//! call** for every typed input: `ValidateProbe(&input).maybe_validate()?` runs
//! `Validate::validate` when `T: Validate`, and is a no-op for any other type.
//! The dispatch is compile-time (an inherent method shadows the trait fallback),
//! so a non-`Validate` argument costs nothing.
//!
//! Bring the fallback trait into scope at the call site for the resolution to
//! work: `use nest_rs_pipes::MaybeValidateFallback as _;`.

use validator::Validate;

use crate::PipeError;

/// Wraps a borrowed input so method resolution can pick the `Validate`-aware
/// inherent method over the no-op trait fallback.
pub struct ValidateProbe<'a, T>(pub &'a T);

/// Specialized path: an **inherent** method (higher priority than the trait
/// fallback) exists only when `T: Validate`, so this runs the real validation.
impl<T: Validate> ValidateProbe<'_, T> {
    pub fn maybe_validate(&self) -> Result<(), PipeError> {
        match self.0.validate() {
            Ok(()) => Ok(()),
            Err(errors) => Err(PipeError::with_details(
                "validation failed",
                serde_json::to_value(errors).unwrap_or(serde_json::Value::Null),
            )),
        }
    }
}

/// Fallback path: available for **every** `T`, so an input type without
/// `Validate` resolves here and skips validation. Shadowed by the inherent
/// method above whenever `T: Validate`.
pub trait MaybeValidateFallback {
    fn maybe_validate(&self) -> Result<(), PipeError>;
}

impl<T> MaybeValidateFallback for ValidateProbe<'_, T> {
    fn maybe_validate(&self) -> Result<(), PipeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The fallback trait must be in scope for the call to resolve — exactly what
    // the transport macros emit (`use MaybeValidateFallback as _;`).
    use super::MaybeValidateFallback as _;

    #[derive(Validate)]
    struct Guarded {
        #[validate(length(min = 1))]
        name: String,
    }

    struct Plain {
        _name: String,
    }

    #[test]
    fn a_validate_type_that_passes_is_ok() {
        let ok = Guarded { name: "x".into() };
        assert!(ValidateProbe(&ok).maybe_validate().is_ok());
    }

    #[test]
    fn a_validate_type_that_fails_surfaces_the_error_with_details() {
        let bad = Guarded {
            name: String::new(),
        };
        let Err(err) = ValidateProbe(&bad).maybe_validate() else {
            panic!("empty name must fail the length rule");
        };
        assert_eq!(err.message(), "validation failed");
        assert!(err.details().is_some(), "field-level details are carried");
    }

    #[test]
    fn a_non_validate_type_is_a_no_op() {
        // `Plain` does not implement `Validate`; the fallback runs and passes.
        let plain = Plain {
            _name: "anything".into(),
        };
        assert!(ValidateProbe(&plain).maybe_validate().is_ok());
    }
}
