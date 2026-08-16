//! The third HTTP emitter, and the one that carried no snapshot.
//!
//! A `#[use_guards]` on the `#[gateway]` struct runs on the **upgrade**, which
//! is an HTTP `GET` — so it owes `HttpGuard`, not `WsGuard`, and the error has
//! to point at `#[gateway]` rather than at `#[messages]`: the guard is written
//! on the struct, and that is the line the author is looking at.
//!
//! Its sibling `http_only_guard_on_a_message` is the mirror image — a guard that
//! *does* check HTTP, refused per message where `check_ws_message` is what runs.
//! Between them the two scopes cannot be confused for one another.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::Guard;
use nest_rs_ws::{gateway, messages};

#[injectable]
#[derive(Default)]
struct UnattestedGuard;

impl Layer for UnattestedGuard {}

// Empty: every `check_*` defaults to `Ok(())`, so this compiles and passes every
// request. The marker is the only thing standing between that and a silent
// open door.
impl Guard for UnattestedGuard {}

#[gateway(path = "/ws")]
#[use_guards(UnattestedGuard)]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
