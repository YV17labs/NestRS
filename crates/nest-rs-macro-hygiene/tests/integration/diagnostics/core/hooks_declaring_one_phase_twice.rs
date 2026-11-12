//! One phase written twice is two declarations of one role, refused by name — not
//! the second copy left behind for rustc's `cannot find attribute`.

use nest_rs::core::hooks;

struct Svc;

#[hooks]
impl Svc {
    #[on_module_init]
    #[on_module_init]
    async fn init(&self) {}
}

fn main() {}
