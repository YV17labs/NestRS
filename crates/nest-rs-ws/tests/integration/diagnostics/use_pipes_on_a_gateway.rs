//! `#[use_pipes]` is HTTP-only, and it is refused by name here.
//!
//! It reached rustc as `cannot find attribute \`use_pipes\` in this scope` —
//! no transport named, no reason, no remedy — while its two neighbours
//! `#[use_interceptors]` and `#[use_filters]` were refused properly.
//! `framework.md` item 8 asks for the named error on **every** layer family the
//! edge does not bridge, and the list was two of four.

use nest_rs_ws::{gateway, messages};

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
