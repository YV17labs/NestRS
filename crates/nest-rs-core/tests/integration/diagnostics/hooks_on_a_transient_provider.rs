//! The worst of the four shapes, and the reason this is a bound rather than a
//! better `warn`: `Container::get` on a `scope = transient` provider **builds a
//! throwaway**, so the hook *runs*, mutates an instance nobody else holds, and
//! is dropped — no skip, no warning, no symptom. Measured before this refusal
//! existed: the init hook ran once and its effect vanished.

use nest_rs_core::{hooks, injectable};

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

#[hooks]
impl PerResolution {
    #[on_module_init]
    async fn init(&self) {}
}

fn main() {}
