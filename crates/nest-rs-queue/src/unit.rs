//! The canonical name of the unit of work this edge opens.
//!
//! Declared by the **contract** crate, which is also the one that opens it: a
//! backend runs the job through [`consume::attempt`](crate::consume::attempt),
//! so the span and the line are the port's semantics, never the adapter's —
//! [`nest_rs_core::unit!`] refuses a declaration from any other crate, and the
//! macros that read a unit refuse a crate that did not declare it. The grammar
//! — `<edge>.<unit>`, lowercase, one dot, the namespace from the closed edge
//! vocabulary — belongs to [`nest_rs_core::operation_log`].

use nest_rs_core::operation_log::Unit;

/// One queue job attempt.
pub const JOB: Unit = nest_rs_core::unit!("queue.job", target: crate::TARGET, kind: Consumer);
