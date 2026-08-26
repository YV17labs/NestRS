//! A probe method must be `async`. The refusal is `nest_rs_codegen`'s shared
//! `must_be_async`, which three provider-hosted decorators impose — and which
//! had a snapshot at none of its three sites, so its wording was pinned nowhere.
//! `#[listeners]` and `#[hooks]` still owe theirs.

use nest_rs_core::injectable;
use nest_rs_health::indicators;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[readiness]
    fn blocking(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
