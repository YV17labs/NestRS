//! `bind = Service` is refused by name here too.
//!
//! The refusal is the half that makes *one declaration, every site the
//! standard permits* affordable — what an unsupported site owes is a
//! *sentence*, not an implementation — and it had **no snapshot at either of
//! its two sites**, so the sentence that carries the whole asymmetry was pinned
//! by nothing.

use nest_rs::ws::{gateway, messages};

struct Update;
struct ChatService;
mod messages_entity {
    pub struct Entity;
}

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("send")]
    #[authorize(Update, bind = ChatService)]
    async fn send(&self) {}
}

fn main() {}
