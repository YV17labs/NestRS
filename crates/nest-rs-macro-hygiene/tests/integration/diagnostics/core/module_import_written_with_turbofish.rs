//! A module of a named instance is imported as its type, `ShelfModule<Cache>`;
//! written with the turbofish the published release taught, it is refused at
//! its `::`, naming the type to write. The type spelling beside it says nothing.

use std::marker::PhantomData;

use nest_rs::core::{ContainerBuilder, Instance, Module, Registering, Unnamed, instance, module};

struct ShelfModule<I = Unnamed>(PhantomData<fn() -> I>);

impl<I: Instance> Module for ShelfModule<I> {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[instance]
struct Cache;

#[module(imports = [ShelfModule<Cache>, ShelfModule::<Cache>])]
struct AppModule;

fn main() {}
