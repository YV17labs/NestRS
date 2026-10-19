//! A host writing rmcp's `ServerHandler` by hand declares no operation, and a
//! host's layers run on the operations `#[tools]` declares: each layer and
//! value bound beside such a host's `#[mcp]` would run nowhere — its guard
//! would gate nothing — so each is refused at its attribute.

use nest_rs::core::{
    Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, UnitContext, injectable,
};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::mcp::{McpView, ServerHandler, mcp};

#[injectable]
#[derive(Default)]
struct Shut;

impl Layer for Shut {}

#[async_trait]
impl Guard for Shut {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("shut"))
    }
}

impl GuardFor<McpView> for Shut {}

#[injectable]
#[derive(Default)]
struct Around;

impl Layer for Around {}

#[async_trait]
impl Interceptor for Around {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[injectable]
#[derive(Default)]
struct Declines;

impl Layer for Declines {}

#[async_trait]
impl ExceptionFilter for Declines {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Declined
    }
}

struct Limit(u32);

#[mcp(path = "/raw")]
#[use_guards(Shut)]
#[use_interceptors(Around)]
#[use_exception_filters(Declines)]
#[meta(Limit(10))]
#[derive(Clone, Default)]
struct RawHost;

impl ServerHandler for RawHost {}

fn main() {}
