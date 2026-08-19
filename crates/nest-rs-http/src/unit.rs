//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, and held to the `<edge>.<unit>`
//! grammar by the `units` join in `nest-rs-conformance`: both are argued once,
//! in [`nest_rs_core::operation_log`].
//!
//! One constant, three slots: the `operation_span!` that opens the unit, the
//! operation line's `name:`, and that line's `message`.

/// One HTTP request, filed once the response body has been written.
pub const REQUEST: &str = "http.request";
