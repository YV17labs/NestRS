//! A layer attribute beside a probe is refused by the layer table: an
//! indicator runs inside the health endpoint, whose layers are the endpoint's.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

struct AllowAll;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[readiness]
    #[use_guards(AllowAll)]
    async fn upstream_reachable(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
