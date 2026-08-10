//! The WS half of the same rule. A guard bound beside a `#[subscribe_message]`
//! runs `check_ws_message`, whose default is `Ok(())`.
//!
//! The remedy the note offers is the one that matters here: move it to the
//! `#[gateway]` struct, where guards run on the HTTP upgrade — a distinction the
//! trait's defaults used to swallow.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard, async_trait};
use nest_rs_ws::{gateway, messages};
use poem::Request;

#[injectable]
#[derive(Default)]
struct HttpOnlyGuard;

impl Layer for HttpOnlyGuard {}

#[async_trait]
impl Guard for HttpOnlyGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Ok(())
    }
}

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    #[use_guards(HttpOnlyGuard)]
    async fn ping(&self) {}
}

fn main() {}
