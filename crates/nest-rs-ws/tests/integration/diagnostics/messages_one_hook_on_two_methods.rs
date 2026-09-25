//! A gateway runs one `#[on_connect]`: a second is refused naming both methods,
//! rather than replacing the first in silence.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[on_connect]
    async fn greet(&self) {}

    #[on_connect]
    async fn count(&self) {}
}

fn main() {}
