//! A message handler is called with the frame's payload and nothing else, so a
//! type parameter has nothing to be inferred from.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping<T: Default + ToString>(&self) -> String {
        T::default().to_string()
    }
}

fn main() {}
