//! Code forbidding a lint refuses every `#[allow]` of it an expansion writes
//! inside (`E0453`). A refused method kept with no body — its `cancel` given no
//! name, or an argument's, its instrument in a shape the decorator cannot read
//! — reads its parameters in that body's place under no lint attribute, so its
//! refusal is the one error here too. So does one kept with its body, in every
//! form: the keys it was refused under are left out, and an argument only a
//! predicate or the fallback read would be rustc's unused variable, whose fix
//! breaks the key. A parameter a macro's pattern binds is taken as `_`, its
//! names read by no attribute: a macro written for that one method is then
//! rustc's unused macro, a warning beside the refusal.

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

macro_rules! parcel {
    ($($weight:tt)+) => {
        $($weight)+
    };
}

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

    async fn stale(&self, parcel: String, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(parcel.len())
    }

    #[resilient(carrier, record_if = |_error| !zone.is_empty(), retries = 3)]
    async fn read_by_its_predicate(&self, zone: String) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale, timout = "2s")]
    async fn read_by_its_fallback(&self, parcel: String) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale, fallback_if = |_error| !parcel.is_empty())]
    async fn read_by_no_host(parcel: String) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale)]
    async fn read_beside_an_answer_unnamed(
        &self,
        parcel: String,
    ) -> Result<impl std::fmt::Display, ResilienceError<CarrierDown>> {
        let weighed: Result<usize, CarrierDown> = Err(CarrierDown);
        Ok(weighed?)
    }

    #[resilient(carrier, fallback = stale)]
    fn read_by_a_fn(&self, parcel: String) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(carrier, cancel)]
    async fn bound_by_a_macro(&self, parcel!(mut weight): u32) -> Answer<usize> {
        if token.is_cancelled() {
            return Err(CarrierDown);
        }
        weight += 1;
        Ok(weight as usize)
    }
}

fn main() {}
