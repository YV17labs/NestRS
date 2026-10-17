//! A probe attribute is a flag: `#[readiness(timeout = "5s")]` compiled and meant
//! nothing, and is refused naming the fact, as `#[hooks]`' phases are.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[readiness(timeout = "5s")]
    async fn upstream_reachable(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
