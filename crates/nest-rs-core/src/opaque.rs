//! The one sentence every transport tells a client when the reason is none of its
//! business.
//!
//! A handler's error type is the framework's only source for the message that
//! reaches a client, and `Display` is the wrong default for that job: a `DbErr`
//! carries SQL, column names and sometimes row values; a storage error carries a
//! bucket key. The framework's own `ServiceError::Db` is
//! `#[error("database error")]` precisely so this cannot bite — a feature's own
//! error type has no such discipline imposed on it.
//!
//! Each client-facing transport therefore exposes an `Opaque` trait beside its
//! error type — `nest_rs_http::Opaque`, `nest_rs_mcp::Opaque`,
//! `nest_rs_graphql::Opaque`, `nest_rs_ws::Opaque` — whose `opaque()` logs the
//! real error at `error` on that transport's own target and substitutes
//! [`OPAQUE_CLIENT_MESSAGE`].
//!
//! HTTP joined last, and its absence had never been argued: the reasoning above
//! is about what a client may read, and a browser reads an error body exactly as
//! untrusted as a language model reads a tool result. The three non-request
//! edges have no trait and want none — a queue job, a scheduled tick and an
//! event listener have no caller to tell anything.
//!
//! **Why the trait is per transport and only the constant is shared.** The trait's
//! *output* is the transport's error type, and that is what makes `.opaque()?`
//! usable: one impl per transport lets inference pick it from the enclosing
//! function's return type. A single trait generic over the output has three
//! applicable impls, so the receiver no longer decides — and every call site would
//! need a turbofish. The shape that survives is one constant here, one thin impl
//! there.

/// What a client is told when an operation fails for a reason it must not read.
///
/// Constant on purpose: a message assembled from the error would leak by
/// construction. Shared so a client cannot tell from the wording which transport
/// it hit, and so a change to that wording cannot land on one edge and miss the
/// others.
pub const OPAQUE_CLIENT_MESSAGE: &str = "internal error";
