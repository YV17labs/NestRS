//! A module never runs another module's phase by hand: only an import does,
//! which runs a module's `collect` before its `register`, each once. The proof
//! a phase receives serves its own module alone, and only the framework makes
//! one.

use nest_rs::core::{ContainerBuilder, Imported, Module, module};

#[module]
struct InnerModule;

struct OuterModule;

impl Module for OuterModule {
    fn register(builder: ContainerBuilder, imported: Imported<Self>) -> ContainerBuilder {
        <InnerModule as Module>::register(builder, imported)
    }

    fn collect(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        <InnerModule as Module>::collect(builder, Imported::new())
    }
}

fn main() {}
