//! Layer registration — typed specs the builder uses to seed the global
//! layer chain into the container. Each transport's shaper resolves them
//! against the live container at configure time.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_pipes::GlobalPipe;

use crate::Guard;

/// One entry in the `use_guards_global` list. Created by [`guard::<T>()`];
/// resolved against the live container at configure time.
pub struct GuardSpec {
    pub type_id: TypeId,
    pub name: &'static str,
    pub(crate) resolve: fn(&Container) -> Option<Arc<dyn Guard>>,
}

/// Construct a [`GuardSpec`] for the given guard type.
///
/// Use inside `App::builder().use_guards_global([...])` to declare which
/// guards run on every request across all transports.
///
/// ```rust,ignore
/// App::builder()
///     .use_guards_global([guard::<AuthGuard>(), guard::<AuthzGuard>()])
///     .module::<AppModule>()
/// ```
pub fn guard<G: Guard + 'static>() -> GuardSpec {
    GuardSpec {
        type_id: TypeId::of::<G>(),
        name: std::any::type_name::<G>(),
        resolve: |c| c.get::<G>().map(|arc| arc as Arc<dyn Guard>),
    }
}

impl GuardSpec {
    /// Resolve this spec against the live container.
    pub fn resolve(&self, container: &Container) -> Option<Arc<dyn Guard>> {
        (self.resolve)(container)
    }
}

/// One entry in the `use_pipes_global` list — same shape as [`GuardSpec`].
pub struct PipeSpec {
    pub type_id: TypeId,
    pub name: &'static str,
    pub(crate) resolve: fn(&Container) -> Option<Arc<dyn GlobalPipe>>,
}

/// Construct a [`PipeSpec`] for the given pipe type.
///
/// ```rust,ignore
/// App::builder()
///     .use_pipes_global([pipe::<StripUnknownFields>()])
///     .module::<AppModule>()
/// ```
pub fn pipe<P: GlobalPipe + 'static>() -> PipeSpec {
    PipeSpec {
        type_id: TypeId::of::<P>(),
        name: std::any::type_name::<P>(),
        resolve: |c| c.get::<P>().map(|arc| arc as Arc<dyn GlobalPipe>),
    }
}

impl PipeSpec {
    pub fn resolve(&self, container: &Container) -> Option<Arc<dyn GlobalPipe>> {
        (self.resolve)(container)
    }
}

/// The unresolved `Vec<GuardSpec>` seeded into the container by
/// `AppBuilder::use_guards_global(...)`. Each transport reads it at
/// configure time and resolves against the live container.
pub struct GuardSpecs(pub Vec<GuardSpec>);

impl GuardSpecs {
    /// Resolve every spec into the composed global chain — deduped and
    /// priority-ordered through the same `compose_chain` as every other
    /// Layer System site. For the single-site consumers that execute the
    /// global pool on their own (the self-mount edge wrap, the GraphQL
    /// fallback operation guard); the per-route shaper builds its own
    /// bucket because it composes against controller / method scopes too.
    pub fn resolve_chain(
        &self,
        container: &Container,
        label: &str,
    ) -> Vec<nest_rs_core::ResolvedLayer<dyn Guard>> {
        let global: Vec<nest_rs_core::ResolvedLayer<dyn Guard>> = self
            .0
            .iter()
            .filter_map(|spec| {
                spec.resolve(container)
                    .map(|layer| nest_rs_core::ResolvedLayer {
                        type_id: spec.type_id,
                        name: spec.name,
                        source: nest_rs_core::LayerSite::Global,
                        layer,
                    })
            })
            .collect();
        nest_rs_core::compose_chain(global, Vec::new(), Vec::new(), &[], label)
    }
}

/// The unresolved `Vec<PipeSpec>` seeded by `AppBuilder::use_pipes_global`.
pub struct PipeSpecs(pub Vec<PipeSpec>);
