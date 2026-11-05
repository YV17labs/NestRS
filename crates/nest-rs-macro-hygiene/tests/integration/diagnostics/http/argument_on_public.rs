//! `#[public]` alone is a posture, and `#[public(Action, Subject)]` names what
//! it reads. A lone word beside it — `read_only`, plausible as a flag — is neither,
//! so it is refused naming what the attribute takes, never dropped into an
//! ungated, unmasked operation with the compiler silent.

use nest_rs::http::{controller, routes};

#[controller(path = "/demo")]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/ping")]
    #[public(read_only)]
    async fn ping(&self) -> String {
        "pong".to_owned()
    }
}

fn main() {}
