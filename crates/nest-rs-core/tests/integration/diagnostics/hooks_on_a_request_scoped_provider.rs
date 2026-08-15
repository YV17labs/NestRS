//! A provider that *is* injectable and still cannot host lifecycle methods:
//! `scope = request` registers a factory, so `Container::get` outside a request
//! answers `None` and the phase is skipped — behind a `warn` that used to name
//! the module tree, which is not the reason. The scope is known where
//! `#[injectable]` reads it, so the refusal belongs at compile time.

use nest_rs_core::{hooks, injectable};

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[hooks]
impl PerRequest {
    #[on_module_init]
    async fn init(&self) {}
}

fn main() {}
