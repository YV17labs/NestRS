//! An `#[interceptor]` is mounted around routes, never held by the container
//! under its own type, so it records no residency and cannot be bound `as dyn
//! Trait`. The refusal is the residency trait's own sentence, which names both
//! readers: the provider-hosted decorators and the `as dyn` binding.

use nest_rs::core::{Layer, module};
use nest_rs::http::interceptor;
use nest_rs::http::poem::{Request, Response, Result};
use nest_rs::interceptors::{Interceptor, Next, async_trait};

trait Stamp: Send + Sync {}

#[interceptor]
#[derive(Default)]
struct Stamped;

impl Layer for Stamped {}

impl Stamp for Stamped {}

#[async_trait]
impl Interceptor for Stamped {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        next.run(req).await
    }
}

#[module(providers = [Stamped as dyn Stamp])]
struct StampModule;

fn main() {}
