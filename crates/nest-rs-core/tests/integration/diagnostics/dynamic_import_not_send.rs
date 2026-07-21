//! CORE-I9 parked the dynamic-import value on the `ContainerBuilder` so the
//! import expression is evaluated once, which added an implicit `Send +
//! 'static` bound. A `for_root(...)` value holding a non-`Send` type therefore
//! fails to compile — this pins the diagnostic so the bound is not silently
//! relaxed.

use std::rc::Rc;

use nest_rs_core::{DynamicModule, module};

struct NotSendModule;

impl NotSendModule {
    fn for_root() -> NotSendSetup {
        NotSendSetup { local: Rc::new(()) }
    }
}

struct NotSendSetup {
    #[allow(dead_code)]
    local: Rc<()>,
}

impl DynamicModule for NotSendSetup {}

#[module(imports = [NotSendModule::for_root()])]
struct AppModule;

fn main() {}
