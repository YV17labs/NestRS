//! An event listener is in-process: there is no wire, so `version = "…"` has no
//! address to name. The refusal says that rather than falling through to
//! `#[listeners]`'s generic "takes no arguments", which reads as a typo in a key
//! that exists somewhere.

use nest_rs_core::injectable;
use nest_rs_events::listeners;

#[injectable]
#[derive(Default)]
struct DemoListener;

#[listeners(version = "1")]
impl DemoListener {}

fn main() {}
