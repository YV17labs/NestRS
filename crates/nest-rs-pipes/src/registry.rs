//! The global pipe pool: [`GlobalPipes`], what the module an app boots
//! declares, and the list each edge resolves from it once, at mount.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::__private::{GlobalLayer, compose_chain, resolve_global_layers};
use nest_rs_core::{Container, LayerSpec};

use crate::GlobalPipe;

/// One pipe a [`GlobalPipes`] lists or a site's `#[use_pipes]` binds. Created
/// by [`pipe::<P>()`](pipe); resolved against the live container when an edge
/// mounts.
pub type PipeSpec = LayerSpec<dyn GlobalPipe>;

/// Construct a [`PipeSpec`] for the given pipe type — what [`GlobalPipes`]
/// declares for each pipe it lists.
pub fn pipe<P: GlobalPipe + 'static>() -> PipeSpec {
    LayerSpec::new(TypeId::of::<P>(), std::any::type_name::<P>(), |c| {
        c.get::<P>().map(|arc| arc as Arc<dyn GlobalPipe>)
    })
}

/// The pipes folded over every JSON HTTP body (a route opts out with
/// `#[no_pipes]`), every `/graphql` operation's variables, sent in a POST or
/// over the graphql-ws socket, and every WebSocket message's `data`, in the
/// order listed — declared once, in the providers of the module the app boots,
/// so the binary and its test suites run the same pool. A module the app
/// imports listing it fails the boot, as does a second declaration. Each entry
/// is a provider, registered here unless the app already registers it, so what
/// it injects is held to the root's imports.
///
/// ```
/// # use nest_rs_core::{App, Layer, injectable, module};
/// use nest_rs_pipes::{GlobalPipe, GlobalPipes};
///
/// #[injectable]
/// #[derive(Layer)]
/// struct StripUnknownFields;
///
/// impl GlobalPipe for StripUnknownFields {}
///
/// #[module(providers = [GlobalPipes<(StripUnknownFields,)>])]
/// struct AppModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// let app = App::builder().module::<AppModule>().build().await?;
/// # Ok(())
/// # }
/// ```
pub struct GlobalPipes<P: GlobalPipeList>(PhantomData<fn() -> P>);

/// A tuple of global pipes — `(StripUnknownFields,)` to eight of them — folded
/// in the order written. Sealed: only tuples are lists.
pub trait GlobalPipeList: listed::Listed + 'static {
    /// Each pipe's spec, in order.
    #[doc(hidden)]
    fn __specs() -> Vec<PipeSpec>;

    /// Each entry, as the provider a pool registers where nothing else did.
    #[doc(hidden)]
    fn __providers() -> Vec<nest_rs_core::__private::PooledProvider>;
}

mod listed {
    pub trait Listed {}
}

nest_rs_core::__global_layer_pool! {
    pool: GlobalPipes,
    list: GlobalPipeList,
    sealed: listed::Listed,
    layer: dyn GlobalPipe,
    each: GlobalPipe => pipe,
}

impl GlobalLayer for dyn GlobalPipe {
    const KIND: &'static str = "pipe";
    const POOL: &'static str = "GlobalPipes";
}

/// The global pool as an edge folds it: resolved once, at mount, composed as
/// every site composes a layer family — one run per type, priority breaking
/// declaration-order ties. Empty when no pool is declared.
pub fn resolve_global_pipes(container: &Container) -> Arc<[Arc<dyn GlobalPipe>]> {
    let global = resolve_global_layers::<dyn GlobalPipe>(container);
    compose_chain(global, Vec::new(), Vec::new(), &[], "the global pipe pool")
        .into_iter()
        .map(|resolved| resolved.layer)
        .collect()
}
