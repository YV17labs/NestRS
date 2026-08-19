//! A hook method declares exactly one lifecycle phase.
//!
//! It took the **first** attribute it found and left the second on the emitted
//! item, where rustc reported "cannot find attribute `on_module_destroy` in this
//! scope" — the framework's own vocabulary presented as if it did not exist,
//! about a method whose init hook had silently been the one that ran.
//! `#[scheduled]` and `#[indicators]` both refuse their second marker by name;
//! this is the third member of that family.

use nest_rs_core::{hooks, injectable};

#[injectable]
#[derive(Default)]
struct Boot;

#[hooks]
impl Boot {
    #[on_module_init]
    #[on_module_destroy]
    async fn both(&self) {}
}

fn main() {}
