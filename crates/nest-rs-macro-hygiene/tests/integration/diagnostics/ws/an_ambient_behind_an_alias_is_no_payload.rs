//! A message handler reads `Ambient<T>` by its name and never as the payload.
//! Behind an alias it would be the payload the client sends, so it is refused
//! naming the fix, beside serde's own refusal of it.

use nest_rs::core::{Ambient, AmbientValue};
use nest_rs::ws::{gateway, messages};

#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

type Me = Ambient<Caller>;

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("whoami")]
    #[public]
    async fn whoami(&self, _caller: Me) -> String {
        String::new()
    }
}

fn main() {}
