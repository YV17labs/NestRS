//! Guard registration — [`GlobalGuards`], the app-wide pool a root module
//! declares, and the scoped specs a decorator declares at its site. Each edge
//! resolves them against the live container when it composes a site.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::__private::{
    GlobalLayer, ResolvedLayer, compose_chain, global_pool_declared, resolve_global_layers,
};
use nest_rs_core::{Container, LayerSpec};

use crate::Guard;

/// One guard declaration: one [`GlobalGuards`] lists, or one a decorator emits
/// for its `#[use_guards]`; resolved against the live container when a site is
/// composed.
pub type GuardSpec = LayerSpec<dyn Guard>;

/// Construct a [`GuardSpec`] for the given guard type — what a guard list
/// (`GlobalGuards`, `EndpointGuards`) declares for each guard it names.
pub fn guard<G: Guard + 'static>() -> GuardSpec {
    LayerSpec::new(TypeId::of::<G>(), std::any::type_name::<G>(), |c| {
        c.get::<G>().map(|arc| arc as Arc<dyn Guard>)
    })
}

/// The guards every unit of every edge runs first, in the order listed —
/// authentication before authorization — declared once, in the providers of
/// the module the app boots, so the binary and its test suites run the same
/// pool. A module the app imports listing it fails the boot, as does a second
/// declaration or a pool out of phase order; a guard the declaring module does
/// not reach fails the access graph.
///
/// The pool takes no capability bound: it runs whichever entry a site calls,
/// `check_http` at a route or a request stage and `check` at an in-band unit.
///
/// ```
/// # use nest_rs_core::{App, Layer, UnitContext, async_trait, injectable, module};
/// # use nest_rs_guards::{Denial, GlobalGuards, Guard};
/// # #[injectable]
/// # #[derive(Default)]
/// # struct AuthnGuard;
/// # impl Layer for AuthnGuard {}
/// # #[async_trait]
/// # impl Guard for AuthnGuard {
/// #     async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
/// #         Ok(())
/// #     }
/// # }
/// # #[injectable]
/// # #[derive(Default)]
/// # struct AuthzGuard;
/// # impl Layer for AuthzGuard {}
/// # #[async_trait]
/// # impl Guard for AuthzGuard {
/// #     async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
/// #         Ok(())
/// #     }
/// # }
/// #[module(providers = [AuthnGuard, AuthzGuard, GlobalGuards<(AuthnGuard, AuthzGuard)>])]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
///
/// let app = App::builder().module::<AppModule>().build().await?;
/// # Ok(())
/// # }
/// ```
pub struct GlobalGuards<G: GlobalGuardList>(PhantomData<fn() -> G>);

/// A tuple of guards — `(AuthnGuard,)` to eight of them — run in the order
/// written. Sealed: only tuples are lists.
pub trait GlobalGuardList: listed::Listed + 'static {
    /// Each guard's spec, in order.
    #[doc(hidden)]
    fn __specs() -> Vec<GuardSpec>;
}

mod listed {
    pub trait Listed {}
}

nest_rs_core::__global_layer_pool! {
    pool: GlobalGuards,
    list: GlobalGuardList,
    sealed: listed::Listed,
    layer: dyn Guard,
    each: Guard => guard,
}

impl GlobalLayer for dyn Guard {
    const KIND: &'static str = "guard";
    const POOL: &'static str = "GlobalGuards";

    fn check(container: &Container) -> Result<(), String> {
        crate::validate::boot_validate_guards(container, &[], "the global guard chain")
    }
}

/// Whether the app declares a global guard pool listing any guard.
pub fn global_guards_active(container: &Container) -> bool {
    global_pool_declared::<dyn Guard>(container)
}

/// The global guard pool resolved into `LayerSite::Global` entries — the one
/// resolution every site composes from, so their dedup inputs cannot drift.
pub fn resolve_global_guards(container: &Container) -> Vec<ResolvedLayer<dyn Guard>> {
    resolve_global_layers::<dyn Guard>(container)
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
