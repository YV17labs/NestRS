//! `unmasked` is refused **by name** on every edge, with the fact that makes it
//! meaningless: no reply mask is left to switch off, since each wire row a reply
//! carries was judged for its caller where `#[expose]`'s conversion built it.

use nest_rs::ws::{gateway, messages};

struct Read;
mod users {
    pub struct Entity;
}

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    #[authorize(Read, users::Entity, unmasked)]
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

fn main() {}
