//! `as dyn Trait` binds one instance for the app's lifetime, built at boot. A
//! `scope = request` provider bound so would hand every request that one
//! instance — per-request state shared across requests — and its build could
//! run before its dependencies exist. The scope is known where `#[injectable]`
//! reads it, so the binding is refused at compile time, on the provider path.

use nest_rs::core::{injectable, module};

trait Basket: Send + Sync {}

#[injectable(scope = request)]
#[derive(Default)]
struct Cart;

impl Basket for Cart {}

#[module(providers = [Cart as dyn Basket])]
struct CartModule;

fn main() {}
