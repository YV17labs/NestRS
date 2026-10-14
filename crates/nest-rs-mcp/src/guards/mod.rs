//! Ready-made [`McpOperationGuard`](super::guard::McpOperationGuard)
//! implementations: the explicit allow-all, the fail-closed deny-all, and the
//! global guard pool's in-band runner.

mod allow;
mod deny;
mod pool;

pub use allow::AllowAllMcpGuard;
pub(crate) use deny::deny_all;
pub(crate) use pool::GlobalPoolMcpGuard;
