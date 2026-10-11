//! The global pipe pool: what `App::builder().use_pipes_global(...)` registers,
//! and the list each edge resolves from it once, at mount.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::__private::{check_specs_resolvable, compose_chain, resolve_global_layers};
use nest_rs_core::layer_chain::GlobalSpecs;
use nest_rs_core::{AppBuilder, Container, LayerSpec};

use crate::GlobalPipe;

/// One entry in the `use_pipes_global` list. Created by [`pipe::<P>()`](pipe);
/// resolved against the live container when an edge mounts.
pub type PipeSpec = LayerSpec<dyn GlobalPipe>;

/// Construct a [`PipeSpec`] for the given pipe type.
///
/// ```
/// # use nest_rs_core::{App, Layer, injectable, module};
/// use nest_rs_pipes::{AppBuilderPipesExt, GlobalPipe, pipe};
///
/// #[injectable]
/// #[derive(Default)]
/// struct StripUnknownFields;
///
/// impl Layer for StripUnknownFields {}
///
/// impl GlobalPipe for StripUnknownFields {}
///
/// #[module(providers = [StripUnknownFields])]
/// struct AppModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// App::builder()
///     .use_pipes_global([pipe::<StripUnknownFields>()])
///     .module::<AppModule>()
/// #   .build().await?;
/// # Ok(())
/// # }
/// ```
pub fn pipe<P: GlobalPipe + 'static>() -> PipeSpec {
    LayerSpec::new(TypeId::of::<P>(), std::any::type_name::<P>(), |c| {
        c.get::<P>().map(|arc| arc as Arc<dyn GlobalPipe>)
    })
}

/// The unresolved `Vec<PipeSpec>` seeded by
/// [`use_pipes_global`](AppBuilderPipesExt::use_pipes_global).
pub struct PipeSpecs(pub Vec<PipeSpec>);

impl GlobalSpecs for PipeSpecs {
    type Layer = dyn GlobalPipe;
    fn specs(&self) -> &[PipeSpec] {
        &self.0
    }
}

/// Adds `.use_pipes_global(...)` to [`AppBuilder`].
///
/// The example on [`pipe`](fn@pipe) registers through it.
pub trait AppBuilderPipesExt: Sized {
    /// Register `specs` as the global pipe pool, folded over every JSON HTTP
    /// body (a route opts out with `#[no_pipes]`), every `/graphql` POST
    /// operation's variables and every WebSocket message's `data`. An
    /// operation sent over the graphql-ws socket, a subscription included,
    /// runs none of it: bind a per-argument pipe there.
    fn use_pipes_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>;
}

impl AppBuilderPipesExt for AppBuilder {
    fn use_pipes_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>,
    {
        self.provide(PipeSpecs(specs.into_iter().collect()))
            .provide_wiring("nest_rs::pipes::global", |container| {
                // At the wiring step, so an app serving no HTTP refuses it too.
                check_specs_resolvable::<PipeSpecs>(
                    container,
                    "pipe",
                    "an unresolvable global pipe would silently drop its edge validation",
                )
                .map_err(nest_rs_core::anyhow::Error::msg)
            })
    }
}

/// The global pool as an edge folds it: resolved once, at mount, composed as
/// every site composes a layer family — one run per type, priority breaking
/// declaration-order ties. Empty when no pool is registered.
pub fn resolve_global_pipes(container: &Container) -> Arc<[Arc<dyn GlobalPipe>]> {
    let global = resolve_global_layers::<PipeSpecs>(container);
    compose_chain(global, Vec::new(), Vec::new(), &[], "the global pipe pool")
        .into_iter()
        .map(|resolved| resolved.layer)
        .collect()
}
