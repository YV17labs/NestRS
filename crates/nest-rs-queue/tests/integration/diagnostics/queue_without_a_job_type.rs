//! `#[queue]`'s second required key. Its sibling `queue_without_a_name` pins
//! the first; both take `nest_rs_codegen::missing_argument`, so a reader meets
//! one sentence whichever key they left out.

use nest_rs_queue::queue;

#[queue(name = "emails")]
pub struct Emails;

fn main() {}
