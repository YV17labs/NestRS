//! Guard registration — the typed specs the builder seeds the global guard
//! chain with. Each transport's shaper resolves them against the live
//! container at configure time.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{Container, LayerSpec};

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
    /// pool on their own (the self-mount edge wrap, the pool runners on
    /// `/graphql` and `/mcp`).
    pub fn resolve_chain(
        &self,
        container: &Container,
        label: &str,
    ) -> Vec<nest_rs_core::__private::ResolvedLayer<dyn Guard>> {
        let global = nest_rs_core::__private::resolve_global_layers::<Self>(container);
        nest_rs_core::__private::compose_chain(global, Vec::new(), Vec::new(), &[], label)
    }
}
