//! The canonical names of the units of work this edge opens, declared through
//! [`nest_rs_core::unit!`]. The message is the unit; the connection is a field.

use nest_rs_core::operation_log::Unit;

/// One WS socket opening.
pub const CONNECT: Unit = nest_rs_core::unit!("ws.connect", target: crate::TARGET, kind: Server);
/// One WS socket closing.
pub const DISCONNECT: Unit =
    nest_rs_core::unit!("ws.disconnect", target: crate::TARGET, kind: Server);
/// One WS message.
pub const MESSAGE: Unit = nest_rs_core::unit!("ws.message", target: crate::TARGET, kind: Server);
