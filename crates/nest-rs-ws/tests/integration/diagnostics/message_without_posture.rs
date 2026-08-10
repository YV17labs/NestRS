//! The mandatory-posture gate, and it is security-load-bearing: a message nobody
//! declared a posture for must **not compile**, rather than reply with rows no
//! ability ever filtered. Same rule, same reason, as on a `#[query]` and a
//! `#[tool]`.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

fn main() {}
