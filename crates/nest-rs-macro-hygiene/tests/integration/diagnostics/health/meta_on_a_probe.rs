//! `#[meta]` on a probe is refused by name: one probe is no dispatch site, and
//! nothing would read the value.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

struct Tag;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[liveness]
    #[meta(Tag)]
    async fn alive(&self) {}
}

fn main() {}
