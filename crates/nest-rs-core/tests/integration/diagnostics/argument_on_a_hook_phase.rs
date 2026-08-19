//! A lifecycle phase attribute is a flag: an argument on it is a compile error,
//! not something the expansion drops.
//!
//! The same shared refusal `#[public]` gets — `nest_rs_codegen::take_flag_attr`
//! words it once — reached by taking the phase through that helper instead of a
//! hand-rolled `position` + `remove` on the attribute's path, which answers the
//! same for `#[on_module_init]` and `#[on_module_init(order = 2)]`.

use nest_rs_core::{hooks, injectable};

#[injectable]
#[derive(Default)]
struct Boot;

#[hooks]
impl Boot {
    #[on_module_init(order = 2)]
    async fn ready(&self) {}
}

fn main() {}
