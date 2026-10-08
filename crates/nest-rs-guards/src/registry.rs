//! Layer registration — typed specs the builder uses to seed the global
//! layer chain into the container. Each transport's shaper resolves them
//! against the live container at configure time.
//!
//! `GuardSpec` and `PipeSpec` are [`LayerSpec`] aliases.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{Container, LayerSpec};
use nest_rs_pipes::GlobalPipe;

use crate::Guard;

/// One entry in the `use_guards_global` list. Created by [`guard::<T>()`](guard);
/// resolved against the live container at configure time.
pub type GuardSpec = LayerSpec<dyn Guard>;

/// Construct a [`GuardSpec`] for the given guard type.
///
/// Use inside `App::builder().use_guards_global([...])` to declare which
/// guards run on every request across all transports.
///
/// ```
/// # use nest_rs_core::{App, Layer, injectable, module};
/// # use nest_rs_guards::{AppBuilderGuardsExt, Guard, guard};
/// # #[injectable]
/// # #[derive(Default)]
/// # struct AuthnGuard;
/// # impl Layer for AuthnGuard {}
/// # impl Guard for AuthnGuard {}
/// # #[injectable]
/// # #[derive(Default)]
/// # struct AuthzGuard;
/// # impl Layer for AuthzGuard {}
/// # impl Guard for AuthzGuard {}
/// # #[module(providers = [AuthnGuard, AuthzGuard])]
/// # struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// App::builder()
///     .use_guards_global([guard::<AuthnGuard>(), guard::<AuthzGuard>()])
///     .module::<AppModule>()
/// #   .build().await?;
/// # Ok(())
/// # }
/// ```
pub fn guard<G: Guard + 'static>() -> GuardSpec {
    LayerSpec::new(TypeId::of::<G>(), std::any::type_name::<G>(), |c| {
        c.get::<G>().map(|arc| arc as Arc<dyn Guard>)
    })
}

/// One entry in the `use_pipes_global` list — same shape as [`GuardSpec`].
pub type PipeSpec = LayerSpec<dyn GlobalPipe>;

/// Construct a [`PipeSpec`] for the given pipe type.
///
/// ```
/// # use nest_rs_core::{App, Layer, injectable, module};
/// # use nest_rs_guards::{AppBuilderPipesExt, pipe};
/// # use nest_rs_pipes::GlobalPipe;
/// # #[injectable]
/// # #[derive(Default)]
/// # struct StripUnknownFields;
/// # impl Layer for StripUnknownFields {}
/// # impl GlobalPipe for StripUnknownFields {}
/// # #[module(providers = [StripUnknownFields])]
/// # struct AppModule;
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

/// The unresolved `Vec<GuardSpec>` seeded into the container by
/// `AppBuilder::use_guards_global(...)`. Each transport reads it at
/// configure time and resolves against the live container.
pub struct GuardSpecs(pub Vec<GuardSpec>);

impl nest_rs_core::layer_chain::GlobalSpecs for GuardSpecs {
    type Layer = dyn Guard;
    fn specs(&self) -> &[GuardSpec] {
        &self.0
    }
}

impl GuardSpecs {
    /// Resolve every spec into the composed global chain — deduped and
    /// priority-ordered through the same `compose_chain` as every other
    /// Layer System site — for the single-site consumers that run the global
    /// pool on their own (the self-mount edge wrap, the fallback operation guards).
    pub fn resolve_chain(
        &self,
        container: &Container,
        label: &str,
    ) -> Vec<nest_rs_core::ResolvedLayer<dyn Guard>> {
        let global = nest_rs_core::layer_chain::resolve_global_layers::<Self>(container);
        nest_rs_core::compose_chain(global, Vec::new(), Vec::new(), &[], label)
    }
}

/// The unresolved `Vec<PipeSpec>` seeded by `AppBuilder::use_pipes_global`.
pub struct PipeSpecs(pub Vec<PipeSpec>);

impl nest_rs_core::layer_chain::GlobalSpecs for PipeSpecs {
    type Layer = dyn GlobalPipe;
    fn specs(&self) -> &[PipeSpec] {
        &self.0
    }
}
