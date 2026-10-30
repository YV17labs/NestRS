//! The WS half of the same rule. A guard bound beside a `#[subscribe_message]`
//! runs `check` on the message, a unit its HTTP decision was not written for.
//!
//! The refusal names the HTTP view the guard does declare: a guard checking only
//! the request belongs on the `#[gateway]` struct, whose guards run on the HTTP
//! upgrade.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::http::HttpView;
use nest_rs::ws::{gateway, messages};

#[injectable]
#[derive(Default)]
struct HttpOnlyGuard;

impl Layer for HttpOnlyGuard {}

#[async_trait]
impl Guard for HttpOnlyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for HttpOnlyGuard {}

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
