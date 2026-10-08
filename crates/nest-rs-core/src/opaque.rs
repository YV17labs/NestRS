//! The one sentence every transport tells a client when the reason is none of its
//! business.
//!
//! Each client-facing transport has its own `Opaque` trait, whose `opaque()` logs
//! the real error and substitutes [`OPAQUE_CLIENT_MESSAGE`]: one trait per
//! transport lets `.opaque()?` infer its output from the enclosing return type.

/// What a client is told when an operation fails for a reason it must not read.
///
/// Constant on purpose: a message assembled from the error would leak by
/// construction.
pub const OPAQUE_CLIENT_MESSAGE: &str = "internal error";
