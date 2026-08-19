//! The canonical names of the units of work this edge opens.
//!
//! Declared here rather than in the kernel, and held to the `<edge>.<unit>`
//! grammar by the `units` join in `nest-rs-conformance`: both are argued once,
//! in [`nest_rs_core::operation_log`].
//!
//! A gateway dispatches *inside* the connection, so the message is the unit and
//! the connection is a field. Both lifecycle hooks are units too: a hook is
//! developer code that logs and writes like any handler.

/// One WS socket opening.
pub const CONNECT: &str = "ws.connect";
/// One WS socket closing.
pub const DISCONNECT: &str = "ws.disconnect";
/// One WS message.
pub const MESSAGE: &str = "ws.message";
