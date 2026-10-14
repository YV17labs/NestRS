//! A guard bound where it does not check is the fail-open this bound closes.
//!
//! `Guard::check_http` admits by default like `Guard::check`, so without the
//! `GuardFor<HttpView>` bound an empty `impl Guard for X {}` beside a verb
//! compiled, read as a protection, and passed every request.

use nest_rs::core::{Layer, injectable};
use nest_rs::guards::Guard;
use nest_rs::http::{controller, routes};

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
