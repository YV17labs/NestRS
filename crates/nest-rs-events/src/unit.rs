//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, and held to the `<edge>.<unit>`
//! grammar by the `units` join in `nest-rs-conformance`: both are argued once,
//! in [`nest_rs_core::operation_log`].
//!
//! **The unit is one listener invocation, not one `emit`.** A listener is
//! developer code that logs, writes and can panic — the same reading that makes
//! `ws.connect` and `ws.disconnect` units of their own — while an `emit` is the
//! emitter's own line of code, already inside whatever unit the emitter is
//! serving. Keying on the listener is also what lets an operator answer the
//! question they actually have: *did the notification listener run for this
//! order, and what did it cost?*

/// One listener invocation for one emitted event.
pub const DISPATCH: &str = "events.dispatch";
