//! Layer chain composition — the dedup-by-`TypeId` logic shared by every
//! execution site of the Layer System (the per-route shaper, the transport
//! pool folds, the GraphQL / WS in-band chains).
//!
//! Two kinds of sources feed a chain:
//!
//! - **Global** specs from `App::builder().use_*_global(...)`.
//! - **Per-route** layers a shaper macro emitted from `#[use_guards]` /
//!   `#[use_pipes]` / etc. on the controller and method.
//!
//! [`ResolvedLayer`] tags each entry with its [`LayerSite`]; the chain
//! builder picks the broadest site for any duplicated [`TypeId`] and runs
//! entries in **declaration order** within the kind, with [`Layer::priority`]
//! as the optional intra-kind tiebreaker. Cross-kind ordering is fixed by
//! the framework (one kind per chain) — there is no "category" reordering.
//!
//! The composed chain is a *pool membership* result: every execution site
//! composes the same three buckets, then executes only the sub-chain that
//! belongs to it (e.g. global interceptors execute at the transport edge,
//! controller/method ones at the route). `priority` orders entries *within*
//! a site; the site itself is chosen by the scope, never by priority.

use std::any::TypeId;
use std::sync::Arc;

use crate::layer::Layer;
// Re-exported so downstream call sites (and macro-emitted code) can name the
// site tag through this module without also importing `layer`.
pub use crate::layer::LayerSite;

/// A layer that survived dedup, paired with its origin site and the name
/// the shaper logged at mount.
pub struct ResolvedLayer<L: ?Sized> {
    pub type_id: TypeId,
    pub name: &'static str,
    pub source: LayerSite,
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
/// 1. Dedup by `TypeId` — the broadest site wins, the rest log a `warn`.
/// 2. The broadest-site rule is bypassed for any `TypeId` listed in
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
    let mut seen: Vec<(TypeId, LayerSite)> = Vec::new();

    for source in [LayerSite::Global, LayerSite::Controller, LayerSite::Method] {
        let bucket = match source {
            LayerSite::Global => &global,
            LayerSite::Controller => &controller,
            _ => &method,
        };
        for entry in bucket {
            let forced = force.contains(&entry.type_id);
            if let Some((_, existing)) = seen.iter().find(|(tid, _)| *tid == entry.type_id) {
                if !forced {
                    report_redundant_scope(entry.type_id, *existing, entry.source, entry.name);
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

/// Report a redundant multi-scope declaration **once per process**.
///
/// `compose_chain` runs per route, so a layer declared at both a broad scope
/// (e.g. `global`) and a narrower one (e.g. `controller`) would otherwise warn
/// on every route of that controller — a process-global structural fact spam-
/// emitted as if it were a per-request event. Dedup by `(layer, scope-pair)`
/// so it surfaces a single line at boot. Level is `debug`: it is informative
/// (the controller/method declaration was skipped because a broader scope
/// already covers it) yet fail-secure (broadest wins, the layer still runs
/// exactly once), so it stays out of `warn` — a config lint, not an actionable
/// security event, kept off `warn` to avoid alert fatigue. `#[force_*]` opts a
/// duplicate back into re-running (logged at `info`).
fn report_redundant_scope(type_id: TypeId, existing: LayerSite, skipped: LayerSite, name: &'static str) {
    use std::collections::HashSet;
    use std::sync::{LazyLock, Mutex};

    static SEEN: LazyLock<Mutex<HashSet<(TypeId, LayerSite, LayerSite)>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    // On a poisoned lock, fall back to emitting — a duplicate diagnostic line
    // is harmless; a swallowed one is not.
    let first_time = SEEN
        .lock()
        .map(|mut seen| seen.insert((type_id, existing, skipped)))
        .unwrap_or(true);
    if first_time {
        tracing::debug!(
            target: "nest_rs::layers",
            layer = short_type_name(name),
            kept = existing.label(),
            skipped = skipped.label(),
            "redundant layer declaration deduped — broadest scope wins (`#[force_*]` to re-run)",
        );
    }
}

/// Strip module paths from a `type_name`, keeping the leaf of each segment so
/// generics survive: `a::b::AuthGuard<c::d::JwtStrategy<e::Claims>>` becomes
/// `AuthGuard<JwtStrategy<Claims>>`. Diagnostic-only — the full path adds no
/// information a reader scanning boot logs can act on.
fn short_type_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut segment_start = 0;
    for (i, c) in name.char_indices() {
        if !(c.is_alphanumeric() || c == '_' || c == ':') {
            out.push_str(leaf(&name[segment_start..i]));
            out.push(c);
            segment_start = i + c.len_utf8();
        }
    }
    out.push_str(leaf(&name[segment_start..]));
    out
}

/// The substring after the last `::` of a path segment.
fn leaf(segment: &str) -> &str {
    segment.rsplit("::").next().unwrap_or(segment)
}

/// Drop intra-bucket duplicates by `TypeId`, keeping the first declaration —
/// **silently**. Used to pre-clean the global bucket before it is handed to
/// [`compose_chain`] at a per-route site: the duplicate was already warned
/// about once at the site that executes the global sub-chain; re-warning on
/// every route would be noise.
pub fn dedup_bucket<L: ?Sized>(bucket: Vec<ResolvedLayer<L>>) -> Vec<ResolvedLayer<L>> {
    let mut seen: Vec<TypeId> = Vec::new();
    bucket
        .into_iter()
        .filter(|entry| {
            if seen.contains(&entry.type_id) {
                return false;
            }
            seen.push(entry.type_id);
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Authn;
    impl Layer for Authn {}
    struct Authz;
    impl Layer for Authz {}
    struct Audit;
    impl Layer for Audit {}

    fn entry<L: Layer + 'static>(layer: L, source: LayerSite) -> ResolvedLayer<dyn Layer> {
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
            vec![entry(Authn, LayerSite::Global)],
            vec![],
            vec![entry(Authn, LayerSite::Method)],
            &[],
            "GET /test",
        );
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].source, LayerSite::Global);
    }

    #[test]
    fn declaration_order_survives_when_priorities_tie() {
        let chain = compose_chain::<dyn Layer>(
            vec![
                entry(Authn, LayerSite::Global),
                entry(Authz, LayerSite::Global),
                entry(Audit, LayerSite::Global),
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
            vec![entry(Authn, LayerSite::Global)],
            vec![],
            vec![entry(Authn, LayerSite::Method)],
            &force,
            "x",
        );
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn dedup_bucket_keeps_first_declaration_silently() {
        let bucket = dedup_bucket::<dyn Layer>(vec![
            entry(Authn, LayerSite::Global),
            entry(Authz, LayerSite::Global),
            entry(Authn, LayerSite::Global),
        ]);
        assert_eq!(bucket.len(), 2);
        assert_eq!(bucket[0].name, std::any::type_name::<Authn>());
        assert_eq!(bucket[1].name, std::any::type_name::<Authz>());
    }
}
