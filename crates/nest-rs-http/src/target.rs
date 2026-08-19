//! The span targets **this crate** owns.
//!
//! A module rather than a bare `TARGET` because this crate owns two concerns:
//! the transport itself, and the route table it mounts. Every crate owning
//! exactly one spells it `TARGET` at its root; [`nest_rs_core::target`] is the
//! only other module of this shape.
//!
//! **Owns, not emits.** A target's one job is to name *where* an event came
//! from, so the crate that owns the concern names it and every crate emitting on
//! it reads the constant rather than spelling a second one. [`ROUTES`] is the
//! worked example: the kernel, the GraphQL and MCP self-mounts, the OpenAPI UI
//! and the WS gateway expansion all file under it, and it is declared here
//! because the transport is the crate they already depend on.

/// The HTTP transport: routing, TLS, versioning, request shaping.
pub const HTTP: &str = "nest_rs::http";
/// The mounted route table.
pub const ROUTES: &str = "nest_rs::routes";
