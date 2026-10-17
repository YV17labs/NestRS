//! The span targets this crate owns.

/// The HTTP transport: routing, TLS, versioning, request shaping.
pub const HTTP: &str = "nest_rs::http";
/// The mounted route table.
pub const ROUTES: &str = "nest_rs::routes";
