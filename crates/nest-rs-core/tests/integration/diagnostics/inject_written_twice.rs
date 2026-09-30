//! `#[inject]` is read once per field, and a second copy is refused by name
//! rather than left on the field for rustc to report as an unknown attribute.

use nest_rs_core::injectable;
// The refused expansion leaves the field unread, so this reads as unused.
#[allow(unused_imports)]
use std::sync::Arc;

#[injectable]
#[derive(Default)]
struct Pool;

#[injectable]
struct Repo {
    #[inject(key = "primary")]
    #[inject(key = "replica")]
    pool: Arc<Pool>,
}

fn main() {}
