//! A connection hook runs no interceptor yet, so one bound on it would be a
//! silent no-op, refused by name: the gateway's interceptors run around its
//! messages alone.

use nest_rs::ws::{gateway, messages};

struct Audit;

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[on_disconnect]
    #[use_interceptors(Audit)]
    async fn left(&self) {}

    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
