//! `#[gateway]`'s `version` is parsed by the same grammar `#[controller]`'s is,
//! which is what its own doc comment already claimed. Taken through a bare
//! string it was a *second* grammar wearing one name: this compiled and mounted
//! the gateway at `/va/b/ws`.

use nest_rs_ws::gateway;

#[gateway(path = "/ws", version = "a/b")]
struct ChatGateway;

fn main() {}
