//! A `#[on_module_init]` method takes no type parameter: the expansion calls it with only
//! what its caller carries, so nothing supplies the type — refused naming the
//! parameter, not with rustc's `type annotations needed` at the decorator.

use nest_rs_core::hooks;

struct Svc;

#[hooks]
impl Svc {
    #[on_module_init]
    async fn init<T: Default>(&self) {}
}

fn main() {}
