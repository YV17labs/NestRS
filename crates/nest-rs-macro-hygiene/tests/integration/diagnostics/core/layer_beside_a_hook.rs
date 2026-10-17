//! A layer attribute beside a lifecycle hook is refused by the layer table.
//!
//! It reached rustc as `cannot find attribute \`use_guards\` in this scope`,
//! the framework's own vocabulary presented as unknown: a hook has no caller to
//! gate and no site a layer could wrap.

use nest_rs::core::{hooks, injectable};

struct AllowAll;

#[injectable]
#[derive(Default)]
struct Boot;

#[hooks]
impl Boot {
    #[on_module_init]
    #[use_guards(AllowAll)]
    async fn warm(&self) {}
}

fn main() {}
