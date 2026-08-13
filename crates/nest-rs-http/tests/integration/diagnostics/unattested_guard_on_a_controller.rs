//! The controller-scope half of the same refusal.
//!
//! A `#[use_guards]` on the `#[controller]` struct folds into every route's
//! chain, so it owes the same attestation the per-route one does — and it is
//! `#[controller]`, not `#[routes]`, that must say so: the guard is written on
//! the struct, and that is where the error has to point.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::Guard;
use nest_rs_http::{controller, routes};

#[injectable]
#[derive(Default)]
struct UnattestedGuard;

impl Layer for UnattestedGuard {}

impl Guard for UnattestedGuard {}

#[controller(path = "/demo")]
#[use_guards(UnattestedGuard)]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn main() {}
