//! The canonical names of the units of work this edge opens.
//!
//! Declared here rather than in the kernel, through [`nest_rs_core::unit!`],
//! whose compile-time evaluation holds the `<edge>.<unit>` grammar: both are
//! argued once, in [`nest_rs_core::operation_log`].
//!
//! A gateway dispatches *inside* the connection, so the message is the unit and
//! the connection is a field. Both lifecycle hooks are units too: a hook is
//! developer code that logs and writes like any handler.

use nest_rs_core::operation_log::Unit;

/// One WS socket opening.
pub const CONNECT: Unit = nest_rs_core::unit!("ws.connect", target: crate::TARGET, kind: Server);
/// One WS socket closing.
pub const DISCONNECT: Unit =
    nest_rs_core::unit!("ws.disconnect", target: crate::TARGET, kind: Server);
/// One WS message.
pub const MESSAGE: Unit = nest_rs_core::unit!("ws.message", target: crate::TARGET, kind: Server);
