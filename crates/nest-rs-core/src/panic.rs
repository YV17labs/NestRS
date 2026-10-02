//! Rendering a caught panic payload as a message.
//!
//! Every seam that contains a panic rather than letting it unwind — a queue
//! consumer, the scheduler, the event bus — has to put *something* structured in
//! the log, and `Box<dyn Any + Send>` is what `catch_unwind` hands back. Three
//! crates had written the same downcast ladder, already drifted on the fallback
//! string and on the field name they logged it under; one home keeps the
//! vocabulary uniform, which is the point of a structured field an operator
//! greps across transports.

use std::any::Any;

/// The field name a contained panic is logged under.
///
/// [`contained_panic!`](crate::contained_panic) writes it as `{ FIELD }`, the
/// constant-name form `tracing` has accepted since 0.1.39, and every test that
/// asserts the field reads it from here — so the name is spelled once. This
/// module exists because three crates had written the same downcast ladder and
/// **had already drifted on the fallback string and on the field name they
/// logged it under**; sharing the ladder and the fallback while leaving the name
/// to each site is the half that drifts silently.
pub const FIELD: &str = "panic";

/// Best-effort message from a caught panic payload — the common `&str` /
/// `String` shapes `panic!` / `unwrap` / `expect` produce — with any of serde's
/// sentences in it said without the value
/// ([`DecodeError::redact`](crate::DecodeError::redact)): `.unwrap()` on a
/// failed decode formats serde's error, value included, into the payload, and
/// the payload is the one text of a panic the framework files.
///
/// Logged under [`FIELD`] by [`contained_panic!`](crate::contained_panic), so
/// one query reaches a contained panic whichever transport caught it.
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
/// The one way a seam that contains a panic says so. It writes the field as
/// `{ FIELD }`, so the name is never spelled at a call site, and it writes the
/// message before it, as `tracing`'s own positional form does: a `{ CONST }`
/// field written first after `target:` is not parsed by `tracing`'s grammar,
/// and the error it gives names the message, not the field. Pass the payload as
/// `payload.as_ref()` or `&*payload`, never `&payload` — a `Box<dyn Any + Send>`
/// is itself `Any`, so a borrow of the box unsizes to the box and every
/// downcast misses.
#[macro_export]
macro_rules! contained_panic {
    (target: $target:expr, $payload:expr, $message:literal $(, $($field:tt)*)?) => {
        $crate::tracing::error!(
            target: $target,
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

        // `panic_any(42)` — nothing to render, so say that rather than losing
        // the event.
        let opaque: Box<dyn Any + Send> = Box::new(42u8);
        assert_eq!(panic_message(opaque.as_ref()), "<non-string panic payload>");
    }

    /// `.unwrap()` on a failed decode panics with serde's error in `Debug` form,
    /// the value inside it; the message files without it.
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
