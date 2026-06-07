//! Extension traits that add the global Layer-System APIs to
//! [`AppBuilder`](nest_rs_core::AppBuilder):
//!
//! - [`AppBuilderGuardsExt::use_guards_global`] — register guards once,
//!   applied to every transport.
//! - [`AppBuilderPipesExt::use_pipes_global`] — register
//!   request-body pipes once, applied to every JSON HTTP handler.

use nest_rs_core::AppBuilder;

use crate::registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs};

/// Adds `.use_guards_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs_guards::{AppBuilderGuardsExt, guard};
///
/// App::builder()
///     .use_guards_global([guard::<AuthGuard>(), guard::<AuthzGuard>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
///
/// Declaration order matters — the runtime chain runs in the order you list
/// the guards (with [`Layer::priority`](nest_rs_core::Layer::priority) as an
/// optional tiebreaker). If you list `AuthzGuard` before `AuthGuard` you'll
/// get an authorization check before authentication has attached the
/// principal — usually a bug.
pub trait AppBuilderGuardsExt: Sized {
    fn use_guards_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>;
}

impl AppBuilderGuardsExt for AppBuilder {
    fn use_guards_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>,
    {
        let collected: Vec<GuardSpec> = specs.into_iter().collect();
        validate_order_by_name(&collected);
        self.provide(GuardSpecs(collected))
    }
}

/// Adds `.use_pipes_global(...)` to [`AppBuilder`] — the NestJS
/// `useGlobalPipes` analog. Each pipe runs before every JSON HTTP handler;
/// per-route opt-out via `#[no_pipes]`.
pub trait AppBuilderPipesExt: Sized {
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
    }
}

/// Log a warning if `Authorization`-sounding precedes `Auth`-sounding in
/// the declaration list. Best-effort static name heuristic; ordering at
/// runtime is whatever the dev listed (no auto-reorder).
fn validate_order_by_name(specs: &[GuardSpec]) {
    let mut saw_authz = false;
    for s in specs {
        let name = s.name.to_ascii_lowercase();
        let is_authz = name.contains("authz") || name.contains("ability");
        let is_authn = (name.contains("auth") && !is_authz) || name.contains("authn");
        if saw_authz && is_authn {
            tracing::warn!(
                target: "nest_rs::layers",
                "global guard order looks reversed — `{}` (looks like authn) follows a guard that looks like authz; authn should precede authz",
                s.name,
            );
        }
        if is_authz {
            saw_authz = true;
        }
    }
}
