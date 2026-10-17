//! A connection hook answers `()`. The gateway calls it and moves on, so a
//! future it returned would be dropped without ever being polled, and an error
//! discarded.

use std::future::Future;

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[on_connect]
    fn greet(&self) -> impl Future<Output = ()> {
        async {}
    }

    #[on_disconnect]
    async fn leave(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
