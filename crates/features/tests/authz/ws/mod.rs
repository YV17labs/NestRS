//! `module.rs` is DI wiring covered by `apps/{api,chat}` e2e.
//!
//! Per-message WS authn/authz is now driven by the global Layer System chain
//! (`AuthGuard.check_ws_message` + `AuthzGuard.check_ws_message`), so the
//! standalone `WsAuthGuard` marker that lived here is gone.
