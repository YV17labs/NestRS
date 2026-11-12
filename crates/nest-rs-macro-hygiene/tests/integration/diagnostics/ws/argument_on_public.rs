//! `#[public]` alone is a posture, and `#[public(Action, Subject)]` names what
//! it reads. A lone word beside it — `read_only`, plausible as a flag — is neither,
//! so it is refused naming what the attribute takes, never dropped into an
//! ungated operation with the compiler silent.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    #[public(read_only)]
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

fn main() {}
