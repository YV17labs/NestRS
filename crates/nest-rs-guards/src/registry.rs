//! Guard registration — the typed specs the builder seeds the global guard
//! chain with, and the scoped specs a decorator declares at its site. Each edge
//! resolves them against the live container when it composes a site.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::__private::{ResolvedLayer, compose_chain, resolve_global_layers};
use nest_rs_core::{Container, LayerSpec};

use crate::Guard;

/// One guard declaration: an entry in the `use_guards_global` list, created by
/// [`guard::<T>()`](guard), or one a decorator emits for its `#[use_guards]`;
/// resolved against the live container when a site is composed.
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
/// `AppBuilder::use_guards_global(...)`. Each edge reads it when it composes a
/// site and resolves it against the live container.
pub struct GuardSpecs(pub Vec<GuardSpec>);

impl nest_rs_core::layer_chain::GlobalSpecs for GuardSpecs {
    type Layer = dyn Guard;
    fn specs(&self) -> &[GuardSpec] {
        &self.0
    }
}

/// Whether the app lists a global guard pool at all.
pub fn global_guards_active(container: &Container) -> bool {
    container
        .get::<GuardSpecs>()
        .is_some_and(|specs| !specs.0.is_empty())
}

/// The global guard pool resolved into `LayerSite::Global` entries — the one
/// resolution every site composes from, so their dedup inputs cannot drift.
pub fn resolve_global_guards(container: &Container) -> Vec<ResolvedLayer<dyn Guard>> {
    resolve_global_layers::<GuardSpecs>(container)
}

/// The global pool composed on its own — deduped and priority-ordered through
/// the same `compose_chain` as every other site — for the endpoints that run it
/// at their request (a self-mount's edge, `/graphql`, `/mcp`).
pub fn resolve_global_chain(container: &Container, label: &str) -> Vec<ResolvedLayer<dyn Guard>> {
    compose_chain(
        resolve_global_guards(container),
        Vec::new(),
        Vec::new(),
        &[],
        label,
    )
}
