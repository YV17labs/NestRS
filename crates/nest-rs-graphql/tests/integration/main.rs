//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.

mod context;
mod diagnostics;
mod duplicate_operation;
mod federation;
mod global_pipe;
mod guard;
mod layer_pool;
mod limits;
mod operation;
mod pipe;
mod read_only;
mod resolver;
mod scope;
mod sdl_snapshot;
mod subscription;
