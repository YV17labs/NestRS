//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, through [`nest_rs_core::unit!`],
//! whose compile-time evaluation holds the `<edge>.<unit>` grammar: both are
//! argued once, in [`nest_rs_core::operation_log`].

use nest_rs_core::operation_log::Unit;

/// One scheduled tick.
pub const TICK: Unit = nest_rs_core::unit!("schedule.tick", target: crate::TARGET, kind: Internal);
