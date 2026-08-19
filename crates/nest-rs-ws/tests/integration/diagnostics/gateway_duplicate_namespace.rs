//! `#[gateway]`'s third key is refused twice over like its two siblings.
//!
//! `path` and `version` each refused a repeat; `namespace` was a plain
//! assignment, so `namespace = A, namespace = B` silently kept the last — and a
//! gateway's namespace decides which `WsServer<N>` it fans out on, so the
//! dropped declaration is which sockets a broadcast reaches.

use nest_rs_ws::{gateway, messages};

struct Chat;
struct Presence;

#[gateway(path = "/ws", namespace = Chat, namespace = Presence)]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
