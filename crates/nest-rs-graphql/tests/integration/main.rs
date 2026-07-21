//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.

mod context;
mod diagnostics;
mod global_pipe;
mod guard;
mod layer_pool;
mod limits;
mod pipe;
mod read_only;
mod resolver;
mod scope;
mod sdl_snapshot;
