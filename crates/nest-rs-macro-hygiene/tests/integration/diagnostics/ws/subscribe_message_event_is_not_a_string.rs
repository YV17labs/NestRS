//! `#[subscribe_message]`'s one argument is the event's name, read as a string
//! literal. A bare word, or no argument at all, is refused at what was written,
//! opening with the attribute — never syn's `expected string literal`.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/a")]
struct AGateway;

#[messages]
impl AGateway {
    #[subscribe_message(ping)]
    #[public]
    async fn ping(&self) {}
}

#[gateway(path = "/b")]
struct BGateway;

#[messages]
impl BGateway {
    #[subscribe_message]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
