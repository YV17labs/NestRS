//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.

mod diagnostics;
mod endpoint;
mod guard;
mod mcp_impl;
mod operation;
mod propagate;
mod registry;
mod scope;
