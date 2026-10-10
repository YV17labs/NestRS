//! The sentences every transport tells a client in place of the server's own
//! text: `<CONDITION>_CLIENT_MESSAGE`, one per condition a `5xx` names.
//!
//! Each client-facing transport has its own `Opaque` trait, whose `opaque()` logs
//! the real error and substitutes [`OPAQUE_CLIENT_MESSAGE`]: one trait per
//! transport lets `.opaque()?` infer its output from the enclosing return type.

/// What a client is told when an operation fails for a reason it must not read
/// (a `500`).
///
/// Constant on purpose: a message assembled from the error would leak by
/// construction.
pub const OPAQUE_CLIENT_MESSAGE: &str = "internal error";

/// What a client is told when something the server depends on did not answer
/// (a `503`): RFC 9110 §15.6.4's reason phrase, lowercased as
/// [`OPAQUE_CLIENT_MESSAGE`] is. Whatever the refusal said of the dependency
/// stays on the server.
pub const UNAVAILABLE_CLIENT_MESSAGE: &str = "service unavailable";
