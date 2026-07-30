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

/// Best-effort message from a caught panic payload — the common `&str` /
/// `String` shapes `panic!` / `unwrap` / `expect` produce.
///
/// Log it under the field name **`panic`**, so one query reaches a contained
/// panic whichever transport caught it.
pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>")
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
}
