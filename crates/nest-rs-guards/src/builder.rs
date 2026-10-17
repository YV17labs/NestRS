//! [`AppBuilderGuardsExt::use_guards_global`] — register guards once, run on
//! every edge.

use nest_rs_core::__private::check_specs_resolvable;
use nest_rs_core::AppBuilder;

use crate::registry::{GuardSpec, GuardSpecs};

/// Adds `.use_guards_global(...)` to [`AppBuilder`].
///
/// The example on [`guard`](fn@crate::guard) registers through it.
///
/// Declaration order is the runtime order, with
/// [`Layer::priority`](nest_rs_core::Layer::priority) as an optional tiebreaker:
/// list `AuthnGuard` before `AuthzGuard`.
pub trait AppBuilderGuardsExt: Sized {
    /// Register `specs` as the global guard pool, run in list order at every
    /// site of every edge — order matters (authn before authz).
    fn use_guards_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>;
}

impl AppBuilderGuardsExt for AppBuilder {
    fn use_guards_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>,
    {
        // Each edge reads the pool off the container where it composes a site:
        // a route, a self-mount's request, an in-band operation.
        self.provide(GuardSpecs(specs.into_iter().collect()))
            // At the wiring step, so an app serving no HTTP refuses it too.
            .provide_wiring("nest_rs::guards::global", |container| {
                check_specs_resolvable::<GuardSpecs>(
                    container,
                    "guard",
                    "an unresolvable global guard would silently drop and leave every route \
                     unguarded",
                )
                .and_then(|()| {
                    crate::validate::boot_validate_guards(container, &[], "the global guard chain")
                })
                .map_err(nest_rs_core::anyhow::Error::msg)
            })
    }
}
