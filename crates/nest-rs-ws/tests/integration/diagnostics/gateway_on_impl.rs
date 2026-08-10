//! The converse: `#[gateway]` names the struct only. Reaching for it on the impl
//! block must name the sibling that does belong there — the shape the developer
//! reached for exists, it is just spelled `#[messages]`.

use nest_rs_ws::gateway;

struct DemoGateway;

#[gateway(path = "/ws")]
impl DemoGateway {
    async fn ping(&self) {}
}

fn main() {}
