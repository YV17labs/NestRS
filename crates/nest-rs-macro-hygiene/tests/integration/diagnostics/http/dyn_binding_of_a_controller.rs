//! An edge host is built at mount, so the container holds no instance of it to
//! bind: `#[controller]` records `ProviderResidency::SINGLETON = false`, and a
//! `#[module]` binding `as dyn Trait` reads it. Refused at compile time, on the
//! provider path, naming the edge host as the shape it is.

use nest_rs::core::module;
use nest_rs::http::{controller, routes};

trait Catalog: Send + Sync {}

#[controller(path = "/things")]
struct ThingsController;

#[routes]
impl ThingsController {}

impl Catalog for ThingsController {}

#[module(providers = [ThingsController as dyn Catalog])]
struct ThingsModule;

fn main() {}
