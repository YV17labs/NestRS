//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, through [`nest_rs_core::unit!`],
//! whose compile-time evaluation holds the `<edge>.<unit>` grammar: both are
//! argued once, in [`nest_rs_core::operation_log`].
//!
//! **The unit is one listener invocation, not one `emit`.** A listener is
//! developer code that logs, writes and can panic — the same reading that makes
//! `ws.connect` and `ws.disconnect` units of their own — while an `emit` is the
//! emitter's own line of code, already inside whatever unit the emitter is
//! serving. Keying on the listener is also what lets an operator answer the
//! question they actually have: *did the notification listener run for this
//! order, and what did it cost?*

use nest_rs_core::operation_log::Unit;

/// One listener invocation for one emitted event.
pub const DISPATCH: Unit =
    nest_rs_core::unit!("events.dispatch", target: crate::TARGET, kind: Internal);
