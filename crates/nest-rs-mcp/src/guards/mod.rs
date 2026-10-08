//! Ready-made [`McpOperationGuard`](super::guard::McpOperationGuard)
//! implementations: the explicit allow-all and the fail-closed deny-all.

mod allow;
mod deny;

pub use allow::AllowAllMcpGuard;
pub(crate) use deny::deny_all;
