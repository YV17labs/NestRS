//! A guard bound where it has no `check_*` is the fail-open this bound closes.
//!
//! `Guard::check_http` defaults to `Ok(())` like every other edge's entry, so
//! before the `HttpGuard` bound an empty `impl Guard for X {}` beside a verb
//! compiled, read as a protection, and passed every request. HTTP was the last
//! edge without the attestation.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::Guard;
use nest_rs_http::{controller, routes};

#[injectable]
#[derive(Default)]
struct UnattestedGuard;

impl Layer for UnattestedGuard {}

impl Guard for UnattestedGuard {}

#[controller(path = "/demo")]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/ping")]
    #[use_guards(UnattestedGuard)]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn main() {}
