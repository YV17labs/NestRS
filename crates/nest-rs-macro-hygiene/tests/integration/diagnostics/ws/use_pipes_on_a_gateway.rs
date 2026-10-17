//! `#[use_pipes]` does not run on WebSockets yet, and it is refused by name here.
//!
//! It reached rustc as `cannot find attribute \`use_pipes\` in this scope` —
//! no transport named, no reason, no remedy — while its two neighbours
//! `#[use_interceptors]` and `#[use_filters]` were refused properly.
//! The named error is owed on **every** layer family the edge does not
//! run, and the list was two of four; the layer table now holds every cell.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/chat")]
#[use_pipes(SomePipe)]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
