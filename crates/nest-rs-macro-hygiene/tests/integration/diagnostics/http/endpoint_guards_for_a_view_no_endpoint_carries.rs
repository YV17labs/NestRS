//! Guards declared for a view no endpoint's request carries would run nowhere:
//! a gateway's messages take their guards from `#[use_guards]`, never from an
//! `EndpointGuards`.

use nest_rs::core::{Layer, UnitContext, injectable, module};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::http::{EndpointGuards, HttpView};
use nest_rs::ws::WsView;

#[injectable]
#[derive(Default)]
struct ApiKeyGuard;

impl Layer for ApiKeyGuard {}

#[async_trait]
impl Guard for ApiKeyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for ApiKeyGuard {}

#[module(providers = [ApiKeyGuard, EndpointGuards<WsView, (ApiKeyGuard,)>])]
struct ChatModule;

fn main() {}
