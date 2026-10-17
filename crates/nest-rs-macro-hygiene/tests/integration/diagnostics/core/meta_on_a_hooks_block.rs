//! `#[meta]` on a `#[hooks]` block is refused by name: a hook declares no
//! `Handler`, so nothing would ever read the value.

use nest_rs::core::{hooks, injectable};

struct Tag;

#[injectable]
#[derive(Default)]
struct Boot;

#[hooks]
#[meta(Tag)]
impl Boot {
    #[on_module_init]
    async fn warm(&self) {}
}

fn main() {}
