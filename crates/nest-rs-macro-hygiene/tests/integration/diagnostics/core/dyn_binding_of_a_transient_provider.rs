//! A `scope = transient` provider promises a fresh instance per resolution;
//! bound `as dyn Trait`, every consumer would share the one built at boot. The
//! binding is refused at compile time, on the provider path.

use nest_rs::core::{injectable, module};

trait Draft: Send + Sync {}

#[injectable(scope = transient)]
#[derive(Default)]
struct Sheet;

impl Draft for Sheet {}

#[module(providers = [Sheet as dyn Draft])]
struct SheetModule;

fn main() {}
