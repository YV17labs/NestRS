//! Layer chain composition — the dedup-by-`TypeId` logic shared by every
//! transport shaper.
//!
//! Two sources feed a chain at mount time:
//!
//! - **Global** specs from `App::builder().use_*_global(...)`.
//! - **Per-route** layers the shaper macro emitted from `#[use_guards]` /
//!   `#[use_pipes]` / etc. on the controller and method.
//!
//! [`ResolvedLayer`] tags each entry with its [`LayerSource`]; the chain
//! builder picks the broadest source for any duplicated [`TypeId`] and runs
//! entries in **declaration order** within the kind, with [`Layer::priority`]
//! as the optional intra-kind tiebreaker. Cross-kind ordering is fixed by
//! the framework (one kind per chain) — there is no "category" reordering.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::Layer;

/// Where a layer entry came from. Used by the dedup logic — when the same
/// [`TypeId`] appears at several scopes, the *broadest* one wins.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayerSource {
    Global,
    Controller,
    Method,
}

impl LayerSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Controller => "controller",
            Self::Method => "method",
        }
    }
}

/// A layer that survived dedup, paired with its origin scope and the name
/// the shaper logged at mount.
pub struct ResolvedLayer<L: ?Sized> {
    pub type_id: TypeId,
    pub name: &'static str,
    pub source: LayerSource,
    pub layer: Arc<L>,
}

impl<L: ?Sized> Clone for ResolvedLayer<L> {
    fn clone(&self) -> Self {
        Self {
            type_id: self.type_id,
            name: self.name,
            source: self.source,
            layer: Arc::clone(&self.layer),
        }
    }
}

/// Compose a deduplicated chain from global + per-route entries.
///
/// Behaviour:
///
/// 1. Dedup by `TypeId` — the broadest scope wins, the rest log a `warn`.
/// 2. The broadest-scope rule is bypassed for any `TypeId` listed in
///    `force` — those entries always survive even if the same `TypeId`
///    is global.
/// 3. Stable sort by [`Layer::priority`] only — declaration order survives
///    when priorities tie (the common case). No "category" ordering: the
///    framework runs one kind per chain.
pub fn compose_chain<L>(
    global: Vec<ResolvedLayer<L>>,
    controller: Vec<ResolvedLayer<L>>,
    method: Vec<ResolvedLayer<L>>,
    force: &[TypeId],
    route_label: &str,
) -> Vec<ResolvedLayer<L>>
where
    L: Layer + ?Sized,
{
    let mut entries: Vec<ResolvedLayer<L>> = Vec::new();
    let mut seen: Vec<(TypeId, LayerSource)> = Vec::new();

    for source in [
        LayerSource::Global,
        LayerSource::Controller,
        LayerSource::Method,
    ] {
        let bucket = match source {
            LayerSource::Global => &global,
            LayerSource::Controller => &controller,
            LayerSource::Method => &method,
        };
        for entry in bucket {
            let forced = force.contains(&entry.type_id);
            if let Some((_, existing)) = seen.iter().find(|(tid, _)| *tid == entry.type_id) {
                if !forced {
                    tracing::warn!(
                        target: "nest_rs::layers",
                        layer = entry.name,
                        existing_scope = existing.label(),
                        skipped_scope = entry.source.label(),
                        route = route_label,
                        "layer declared at multiple scopes — broadest wins, later declaration ignored (use `#[force_*]` to force a re-run)",
                    );
                    continue;
                }
                tracing::info!(
                    target: "nest_rs::layers",
                    layer = entry.name,
                    scope = entry.source.label(),
                    route = route_label,
                    "layer forced to re-run despite being declared at a broader scope",
                );
            }
            seen.push((entry.type_id, entry.source));
            entries.push(entry.clone());
        }
    }

    // Stable sort by priority only. Declaration order survives as the
    // tiebreaker when priorities are equal (the common case).
    entries.sort_by_key(|e| e.layer.priority());

    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::Layer;

    struct Authn;
    impl Layer for Authn {}
    struct Authz;
    impl Layer for Authz {}
    struct Audit;
    impl Layer for Audit {}

    fn entry<L: Layer + 'static>(layer: L, source: LayerSource) -> ResolvedLayer<dyn Layer> {
        ResolvedLayer {
            type_id: TypeId::of::<L>(),
            name: std::any::type_name::<L>(),
            source,
            layer: Arc::new(layer) as Arc<dyn Layer>,
        }
    }

    #[test]
    fn dedup_keeps_global_drops_method_for_same_typeid() {
        let chain = compose_chain::<dyn Layer>(
            vec![entry(Authn, LayerSource::Global)],
            vec![],
            vec![entry(Authn, LayerSource::Method)],
            &[],
            "GET /test",
        );
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].source, LayerSource::Global);
    }

    #[test]
    fn declaration_order_survives_when_priorities_tie() {
        let chain = compose_chain::<dyn Layer>(
            vec![
                entry(Authn, LayerSource::Global),
                entry(Authz, LayerSource::Global),
                entry(Audit, LayerSource::Global),
            ],
            vec![],
            vec![],
            &[],
            "x",
        );
        let names: Vec<_> = chain.iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            vec![
                std::any::type_name::<Authn>(),
                std::any::type_name::<Authz>(),
                std::any::type_name::<Audit>(),
            ],
        );
    }

    #[test]
    fn force_replays_layer_despite_global_declaration() {
        let force = vec![TypeId::of::<Authn>()];
        let chain = compose_chain::<dyn Layer>(
            vec![entry(Authn, LayerSource::Global)],
            vec![],
            vec![entry(Authn, LayerSource::Method)],
            &force,
            "x",
        );
        assert_eq!(chain.len(), 2);
    }
}
