//! Code forbidding a lint refuses every `#[allow]` of it an expansion writes
//! inside (`E0453`). A refused method kept with no body — its `cancel` given no
//! name, or an argument's, its instrument in a shape the decorator cannot read
//! — reads its parameters in that body's place under no lint attribute, so its
//! refusal is the one error here too.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[derive(Debug)]
struct CarrierDown;

impl std::fmt::Display for CarrierDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the carrier did not answer")
    }
}

impl std::error::Error for CarrierDown {}

type Answer<T = ()> = Result<T, ResilienceError<CarrierDown>>;

#[injectable]
struct ShipmentsService {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

#[forbid(unused_variables, unused_mut)]
impl ShipmentsService {
    #[resilient(carrier, cancel)]
    async fn unnamed(&self, mut order: String, (zone, weight): (&str, u32)) -> Answer<usize> {
        if token.is_cancelled() {
            return Err(CarrierDown);
        }
        order.push_str(zone);
        Ok(order.len() + weight as usize)
    }

    #[resilient(carrier, cancel = token)]
    async fn named_as_an_argument(&self, token: String) -> Answer<usize> {
        Ok(token.len())
    }

    #[resilient(carrier)]
    async fn unread(&self, slug: String) -> Answer<usize> {
        let __tracing_attr_span = slug.len();
        Ok(__tracing_attr_span)
    }
}

fn main() {}
