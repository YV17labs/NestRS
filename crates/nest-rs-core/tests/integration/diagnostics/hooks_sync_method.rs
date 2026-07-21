//! `#[hooks]` phase methods must be `async fn` — a sync one is a spanned
//! compile error, not a silently-skipped lifecycle hook.

use nest_rs_core::hooks;

struct Svc;

#[hooks]
impl Svc {
    #[on_module_init]
    fn init(&self) {}
}

fn main() {}
