//! A module never runs a phase by hand, its own or another module's: only an
//! import does, which runs a module's `collect` before its `register`, each
//! once. The proof a phase receives serves that phase of its own module alone,
//! and only the framework makes one.

use nest_rs::core::{Collecting, ContainerBuilder, Module, Registering, module};

#[module]
struct InnerModule;

struct OuterModule;

impl Module for OuterModule {
    fn register(builder: ContainerBuilder, registering: Registering<Self>) -> ContainerBuilder {
        <InnerModule as Module>::register(builder, registering)
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        <InnerModule as Module>::collect(builder, Collecting::new())
    }
}

struct ReplayingModule;

impl Module for ReplayingModule {
    fn register(builder: ContainerBuilder, registering: Registering<Self>) -> ContainerBuilder {
        Self::collect(builder, registering)
    }
}

fn main() {}
