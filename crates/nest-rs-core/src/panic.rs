//! Rendering a caught panic payload as a message.

use std::any::Any;

/// The field name a contained panic is logged under.
pub const FIELD: &str = "panic";

/// Best-effort message from a caught panic payload (`&str` or `String`), with
/// serde's sentences redacted ([`DecodeError::redact`](crate::DecodeError::redact)):
/// `.unwrap()` on a failed decode puts the value into the payload.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    crate::DecodeError::redact(message, None).into_owned()
}

/// Log a contained panic: at `error`, on `target`, with the payload rendered by
/// [`panic_message`] under [`FIELD`], then the seam's own fields.
///
/// ```
/// # const TARGET: &str = "nest_rs::fixture";
/// let payload = std::panic::catch_unwind(|| panic!("boom")).expect_err("it panics");
/// nest_rs_core::contained_panic!(
///     target: TARGET,
///     payload.as_ref(),
///     "listener panicked — dispatch continues",
///     listener = "notify",
/// );
/// ```
///
/// Pass `payload.as_ref()` or `&*payload`, never `&payload`: a `Box<dyn Any +
/// Send>` is itself `Any`, so a borrow of the box unsizes to the box and every
/// downcast misses.
#[macro_export]
macro_rules! contained_panic {
    (target: $target:expr, $payload:expr, $message:literal $(, $($field:tt)*)?) => {
        $crate::tracing::error!(
            target: $target,
            // `tracing` does not parse a `{ CONST }` field written first after `target:`.
            message = ::core::format_args!($message),
            { $crate::panic::FIELD } = %$crate::panic_message($payload),
            $($($field)*)?
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_two_shapes_panic_produces_and_names_the_third() {
        let literal: Box<dyn Any + Send> = Box::new("deliberate panic");
        assert_eq!(panic_message(literal.as_ref()), "deliberate panic");

        let formatted: Box<dyn Any + Send> = Box::new(format!("panic for {}", "boom"));
        assert_eq!(panic_message(formatted.as_ref()), "panic for boom");

        let opaque: Box<dyn Any + Send> = Box::new(42u8);
        assert_eq!(panic_message(opaque.as_ref()), "<non-string panic payload>");
    }

    #[test]
    fn a_panic_over_a_decode_failure_is_said_without_its_value() {
        let payload = std::panic::catch_unwind(|| {
            serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).unwrap()
        })
        .expect_err("a secret is not a number");
        let message = panic_message(payload.as_ref());
        assert!(!message.contains("sk_live"), "{message}");
        assert_eq!(
            message,
            "called `Result::unwrap()` on an `Err` value: Error(\"invalid type: a string, \
             expected u64\", line: 1, column: 24)"
        );
    }
}
