//! A module of a named instance is imported as its type, `ShelfModule<Cache>`;
//! written with the turbofish it is deprecated, warned at its `::` naming the
//! type to write, and refused from 8.0. Denied here so the snapshot pins the
//! warning; the type spelling beside it says nothing.
#![deny(deprecated)]

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
