//! The codes a policy's own refusals are known by on every edge.

use nest_rs_core::Code;

/// An attempt outlived the policy's timeout, or the call its deadline: `504`.
pub const TIMEOUT: Code = Code::new("TIMEOUT");

/// The dependency's circuit breaker is open, and the call was not made:
/// `503`, with `Retry-After`.
pub const CIRCUIT_OPEN: Code = Code::new("CIRCUIT_OPEN");

/// Every slot of the dependency's bulkhead was taken for as long as the call
/// could wait for one, and the call was not made: `503`.
pub const BULKHEAD_FULL: Code = Code::new("BULKHEAD_FULL");
