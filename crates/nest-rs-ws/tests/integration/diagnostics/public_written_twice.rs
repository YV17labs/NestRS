//! A decorator read once is refused by name when written twice. The first copy
//! was taken and the second left on the method, where rustc reported
//! `cannot find attribute` — no decorator, no reason, no remedy. One sentence
//! for every such attribute (`nest_rs_codegen::take_single_attr`), pinned at
//! each edge.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    #[public]
    #[public]
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

fn main() {}
