//! A probe method borrows its host: one taking `self` by value is refused quoting
//! the receiver written, not with rustc's `cannot move out of an Arc` about the
//! expansion's call.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[readiness]
    async fn upstream_reachable(self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
