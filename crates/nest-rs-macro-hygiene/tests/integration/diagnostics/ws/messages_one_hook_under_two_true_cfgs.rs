//! Two `#[on_connect]` hooks whose `#[cfg]`s both hold are two hooks for one
//! connection, and the gateway runs one — refused, rather than one replacing
//! the other in silence.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[cfg(all())]
    #[on_connect]
    async fn greet(&self) {}

    #[cfg(all())]
    #[on_connect]
    async fn count(&self) {}
}

fn main() {}
