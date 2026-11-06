//! Validation at the edge, with no wrapper: every edge's tail probes each
//! value it decoded, once its guards and class gate admitted the caller and
//! before any pipe runs over it, and runs the rules the value's type declares.
//!
//! The probe is autoref specialization, resolved where the expansion names a
//! concrete type: `(&&ValidateProbe(&arg)).maybe_validate()` runs
//! `Validate::validate` when the type implements it, validates an `Option`'s
//! value when it holds one, and is a compile-time no-op for any other type.
//! The three traits must be in scope at the call site; the expansion imports
//! them. An HTTP extractor and a list validate as their value does, so the one
//! probe serves every edge.

use validator::{Validate, ValidationErrors};

use crate::PipeError;

/// A `validator` failure as the refusal every edge renders: `validation failed`,
/// its fields under `details`, none of the submitted values
/// ([`validation_details`]).
///
/// A hand-written check answers like a decorated edge through it:
///
/// ```
/// use nest_rs_core::Secret;
/// use nest_rs_pipes::PipeError;
/// use validator::Validate;
///
/// #[derive(Validate)]
/// struct Signup {
///     #[validate(length(min = 8))]
///     password: Secret<String>,
/// }
///
/// let refused = Signup { password: "hunter2".into() }
///     .validate()
///     .map_err(PipeError::from)
///     .unwrap_err();
/// assert_eq!(refused.message(), "validation failed");
/// assert!(!refused.details().unwrap().to_string().contains("hunter2"));
/// ```
impl From<ValidationErrors> for PipeError {
    fn from(errors: ValidationErrors) -> Self {
        PipeError::with_details("validation failed", validation_details(&errors))
    }
}

/// The wire-safe JSON for a `validator` failure.
///
/// A failure keeps its field name, `code` and the `message` its rule declares
/// at every depth — none where the rule declares none, never a `null` a client
/// must handle — and no `params`, which echo submitted input (`params.value`,
/// `params.other`, a bound reading another field). Any layer putting
/// `ValidationErrors` on a wire must use it. An unserializable error map
/// collapses to `Null`.
pub fn validation_details(errors: &ValidationErrors) -> serde_json::Value {
    let mut details = serde_json::to_value(errors).unwrap_or(serde_json::Value::Null);
    redact_submitted_values(&mut details);
    details
}

/// Recursively drop the `params` of serialized `validator` errors, and a
/// `message` no rule declared. Nested (`#[validate(nested)]`) and list
/// validations embed further error maps, so the walk descends through every
/// object and array.
fn redact_submitted_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if matches!(map.get("params"), Some(serde_json::Value::Object(_))) {
                map.remove("params");
            }
            if matches!(map.get("message"), Some(serde_json::Value::Null)) {
                map.remove("message");
            }
            for nested in map.values_mut() {
                redact_submitted_values(nested);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_submitted_values(item);
            }
        }
        _ => {}
    }
}

/// A borrowed decoded argument, wrapped so method resolution picks the most
/// specific of [`DeclaredRules`], [`OptionalRules`] and [`NoRules`].
pub struct ValidateProbe<'a, T>(pub &'a T);

impl<T> Clone for ValidateProbe<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ValidateProbe<'_, T> {}

/// First choice: the argument's type declares rules, which run.
pub trait DeclaredRules {
    /// Run the rules `T` declares.
    fn maybe_validate(&self) -> Result<(), PipeError>;
}

impl<T: Validate> DeclaredRules for &ValidateProbe<'_, T> {
    fn maybe_validate(&self) -> Result<(), PipeError> {
        self.0.validate().map_err(PipeError::from)
    }
}

/// Second choice: an optional argument whose value declares rules, which run
/// when it holds one.
pub trait OptionalRules {
    /// Run the rules the value declares, if there is one.
    fn maybe_validate(&self) -> Result<(), PipeError>;
}

impl<T: Validate> OptionalRules for ValidateProbe<'_, Option<T>> {
    fn maybe_validate(&self) -> Result<(), PipeError> {
        match self.0 {
            Some(value) => value.validate().map_err(PipeError::from),
            None => Ok(()),
        }
    }
}

/// Last choice: a type declaring no rules, which nothing checks.
pub trait NoRules {
    /// Accept the argument.
    fn maybe_validate(self) -> Result<(), PipeError>;
}

impl<T> NoRules for ValidateProbe<'_, T> {
    fn maybe_validate(self) -> Result<(), PipeError> {
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::needless_borrow,
    reason = "each probe is called as an expansion calls it, whichever choice resolves"
)]
mod tests {
    use super::*;

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
        assert!((&&ValidateProbe(&ok)).maybe_validate().is_ok());
    }

    #[test]
    fn a_validate_type_that_fails_surfaces_the_error_with_details() {
        let bad = Guarded {
            name: String::new(),
        };
        let Err(err) = (&&ValidateProbe(&bad)).maybe_validate() else {
            panic!("empty name must fail the length rule");
        };
        assert_eq!(err.message(), "validation failed");
        assert!(err.details().is_some(), "field-level details are carried");
    }

    #[test]
    fn an_optional_value_is_validated_when_it_is_there() {
        let given = Some(Guarded {
            name: String::new(),
        });
        assert!((&&ValidateProbe(&given)).maybe_validate().is_err());
        let absent: Option<Guarded> = None;
        assert!((&&ValidateProbe(&absent)).maybe_validate().is_ok());
        let plain = Some(3_u8);
        assert!((&&ValidateProbe(&plain)).maybe_validate().is_ok());
    }

    #[test]
    fn a_list_is_validated_item_by_item() {
        let listed = vec![
            Guarded { name: "x".into() },
            Guarded {
                name: String::new(),
            },
        ];
        assert!((&&ValidateProbe(&listed)).maybe_validate().is_err());
    }

    #[derive(Validate)]
    struct Transfer {
        ceiling: u64,
        #[validate(range(max = self.ceiling))]
        amount: u64,
    }

    /// A bound is an expression that can read another field, so the details say
    /// the rule and none of its parameters.
    #[test]
    fn the_details_say_the_rule_and_none_of_its_parameters() {
        let transfer = Transfer {
            ceiling: 4_242_424_242,
            amount: 9_999_999_999,
        };
        let Err(errors) = transfer.validate() else {
            panic!("an amount above its ceiling must fail the range rule");
        };
        let details = validation_details(&errors);
        assert_eq!(details["amount"][0]["code"], "range");
        assert!(details["amount"][0].get("params").is_none(), "{details}");
        assert!(!details.to_string().contains("4242424242"), "{details}");
    }

    #[derive(Validate)]
    struct Signup {
        #[validate(email)]
        email: String,
        #[validate(length(min = 8, message = "at least eight characters"))]
        password: String,
    }

    /// A rule declaring no message carries none, never a `null`; one that
    /// declares its message keeps it.
    #[test]
    fn an_entry_carries_the_message_its_rule_declares_or_none() {
        let signup = Signup {
            email: "not-an-email".into(),
            password: "short".into(),
        };
        let Err(errors) = signup.validate() else {
            panic!("both rules refuse");
        };
        let details = validation_details(&errors);
        assert_eq!(details["email"][0]["code"], "email");
        assert!(
            details["email"][0].get("message").is_none(),
            "no `message` member: {details}"
        );
        assert_eq!(
            details["password"][0]["message"],
            "at least eight characters"
        );
    }

    #[test]
    fn a_non_validate_type_is_a_no_op() {
        let plain = Plain {
            _name: "anything".into(),
        };
        assert!((&&ValidateProbe(&plain)).maybe_validate().is_ok());
    }
}
