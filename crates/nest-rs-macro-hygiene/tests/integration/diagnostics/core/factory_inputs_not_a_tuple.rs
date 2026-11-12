//! A factory's inputs are written as a tuple, one input included: a bare
//! type in the inputs position is refused in the words `FactoryInputs` pins.

use nest_rs::core::{Collecting, ContainerBuilder, Module, Registering};

struct Pool;

struct Client;

struct ClientModule;

impl Module for ClientModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory(|_| async { Ok(Pool) })
            .provide_factory_after::<Client, Pool, _, _>(|_| async { Ok(Client) })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

fn main() {}
