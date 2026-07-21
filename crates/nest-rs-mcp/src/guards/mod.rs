//! Ready-made [`McpOperationGuard`](super::guard::McpOperationGuard)
//! implementations for the two default postures — the explicit allow-all and
//! the fail-closed deny-all fallback. The trait itself stays at the parent
//! (`guard.rs`); these are its concrete variants.

mod allow;
mod deny;

pub use allow::AllowAllMcpGuard;
pub(crate) use deny::deny_all;
