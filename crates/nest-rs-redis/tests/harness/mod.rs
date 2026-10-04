//! What both of this crate's suites stand in front of — or in place of — a
//! Redis: [`connection`]'s scripted server and the shape of a refusal, and
//! [`tls`]'s terminating proxy. Each suite's `main.rs` includes it by path; an
//! item here is one both suites use, or the suite that does not would hold
//! dead code.

pub(crate) mod connection;
pub(crate) mod tls;

use std::time::Duration;

/// A refusal is one connection attempt, never a budget spent retrying one.
pub(crate) const AT_ONCE: Duration = Duration::from_secs(3);
