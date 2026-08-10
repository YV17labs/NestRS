//! Response masking is fail-closed, so a masked value the return type can no
//! longer represent has to refuse the message — and a handler returning a bare
//! value has no error channel to refuse through. Named compile error rather than
//! an unmet trait bound deep inside the expansion.

use nest_rs_ws::{gateway, messages};

struct Widget;

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    #[authorize(Read, Widget)]
    async fn list(&self) -> Vec<String> {
        vec![]
    }
}

fn main() {}
