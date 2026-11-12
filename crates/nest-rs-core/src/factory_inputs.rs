//! [`FactoryInputs`] — what a queued factory reads from its snapshot, written
//! as a tuple of the factory outputs it waits on.

use std::any::{Any, TypeId};

/// The factory outputs a factory reads from its snapshot, written as a tuple:
/// `(A,)`, `(A, B)`, `(A, B, C)` or `(A, B, C, D)`. The boot runs the factory
/// once each is present or provided by nothing still queued, wherever the
/// factories fall in `imports = [..]`.
///
/// ```
/// # use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering};
/// struct Pool;
/// struct Settings;
/// struct Client;
///
/// struct ClientModule;
///
/// impl Module for ClientModule {
///     fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
///         builder.provide_factory_after::<Client, (Pool, Settings), _, _>(|_| async { Ok(Client) })
///     }
///
///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
///         builder
///     }
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = App::builder()
/// #     .module::<ClientModule>()
/// #     .provide_factory(|_| async { Ok(Pool) })
/// #     .provide_factory(|_| async { Ok(Settings) })
/// #     .build()
/// #     .await?;
/// # assert!(app.container().get::<Client>().is_some());
/// # Ok(())
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "a factory's inputs are written as a tuple",
    label = "write `({Self},)` for one input",
    note = "a factory reads up to four inputs: `(A,)`, `(A, B)`, `(A, B, C)` or `(A, B, C, D)`"
)]
pub trait FactoryInputs: sealed::Sealed + 'static {
    /// The type of each input, in the order written.
    fn type_ids() -> Vec<TypeId>;
}

// A tuple is the one spelling, so no other type implements it.
mod sealed {
    pub trait Sealed {}
}

impl<A: Any> sealed::Sealed for (A,) {}
impl<A: Any, B: Any> sealed::Sealed for (A, B) {}
impl<A: Any, B: Any, C: Any> sealed::Sealed for (A, B, C) {}
impl<A: Any, B: Any, C: Any, D: Any> sealed::Sealed for (A, B, C, D) {}

impl<A: Any> FactoryInputs for (A,) {
    fn type_ids() -> Vec<TypeId> {
        vec![TypeId::of::<A>()]
    }
}

impl<A: Any, B: Any> FactoryInputs for (A, B) {
    fn type_ids() -> Vec<TypeId> {
        vec![TypeId::of::<A>(), TypeId::of::<B>()]
    }
}

impl<A: Any, B: Any, C: Any> FactoryInputs for (A, B, C) {
    fn type_ids() -> Vec<TypeId> {
        vec![TypeId::of::<A>(), TypeId::of::<B>(), TypeId::of::<C>()]
    }
}

impl<A: Any, B: Any, C: Any, D: Any> FactoryInputs for (A, B, C, D) {
    fn type_ids() -> Vec<TypeId> {
        vec![
            TypeId::of::<A>(),
            TypeId::of::<B>(),
            TypeId::of::<C>(),
            TypeId::of::<D>(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_input_is_listed_in_the_order_written() {
        assert_eq!(<(u8,)>::type_ids(), [TypeId::of::<u8>()]);
        assert_eq!(
            <(u8, u16, u32, u64)>::type_ids(),
            [
                TypeId::of::<u8>(),
                TypeId::of::<u16>(),
                TypeId::of::<u32>(),
                TypeId::of::<u64>(),
            ]
        );
    }
}
