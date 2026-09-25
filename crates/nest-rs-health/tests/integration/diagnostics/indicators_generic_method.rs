//! A `#[readiness]` method takes no type parameter: the expansion calls it with only
//! what its caller carries, so nothing supplies the type — refused naming the
//! parameter, not with rustc's `type annotations needed` at the decorator.

use nest_rs_core::injectable;
use nest_rs_health::indicators;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[readiness]
    async fn upstream_reachable<T: Default>(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
