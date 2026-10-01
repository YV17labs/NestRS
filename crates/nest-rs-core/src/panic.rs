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
/// Unusable at the emit site and declared anyway, for the reason
/// [`operation_log::DURATION_MS`](crate::operation_log::DURATION_MS) is: a field
/// name is a literal token in `tracing`'s macro grammar, so an edge spells it
/// rather than referencing this — and the vocabulary of the line still belongs
/// in one place. This module exists because three crates had written the same
/// downcast ladder and **had already drifted on the fallback string and on the
/// field name they logged it under**; the ladder and the fallback were shared
/// and the name was left in prose, which is the half that drifts silently.
pub const FIELD: &str = "panic";

/// Best-effort message from a caught panic payload — the common `&str` /
/// `String` shapes `panic!` / `unwrap` / `expect` produce — with any of serde's
/// sentences in it said without the value
/// ([`DecodeError::redact`](crate::DecodeError::redact)): `.unwrap()` on a
/// failed decode formats serde's error, value included, into the payload, and
/// the payload is the one text of a panic the framework files.
///
/// Log it under [`FIELD`], so one query reaches a contained panic whichever
/// transport caught it.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    crate::DecodeError::redact(message, None).into_owned()
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
