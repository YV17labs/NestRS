//! Mirrors `src/authz/ws/`. `module.rs` is DI wiring exercised by app e2e
//! (`apps/api`, `apps/chat`); the runtime fail-closed behaviour of
//! [`WsAuthGuard`](features::authz::ws::WsAuthGuard) is tested here.

mod guard;
