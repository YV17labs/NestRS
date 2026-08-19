//! The span targets **this crate** emits on.
//!
//! `CLAUDE.md` fixes the shape — dotted, lowercase, **rooted at the crate that
//! emits them**, the crate picking the root and the concern the tail — and that
//! rule decides where a constant lives as much as what it says. A target's one
//! job is to name *where* an event came from, so the crate that **owns the
//! concern** is the crate that names it: `nest_rs_events::TARGET` belongs to
//! `nest-rs-events`, and a table here holding it would have meant the kernel
//! carrying a name for a concern it does not know exists. The kernel owns six,
//! and declares six.
//!
//! **Owns, not emits, and the difference is real.** `nest_rs::layers` is emitted
//! from here and from three crates above; `nest_rs::routes` from five. A shared
//! concern is declared once by whichever crate the others already depend on —
//! `nest_rs_http::target::ROUTES` for the route table — and read from there. The
//! two are only ever the same crate when a concern has one emitter.
//!
//! It is a module rather than a bare `TARGET` for the same reason
//! `nest-rs-http` has one: a crate owning several concerns needs several names.
//! Every crate owning exactly one spells it `TARGET` at its root.
//!
//! **The operation log is deliberately not here.** It is
//! [`operation_log::TARGET`](crate::operation_log::TARGET), the one target
//! naming a *category of line* rather than a subsystem — which is the whole
//! reason it exists, and is a different thing from a concern several crates
//! emit on.

/// Boot verdicts of the access graph — a resolver no module reaches, a provider injecting what it may not.
pub const ACCESS_GRAPH: &str = "nest_rs::access_graph";
/// Composition root: transports attached, boot phases, shutdown.
pub const APP: &str = "nest_rs::app";
/// Provider registration and resolution, including shadowed bindings.
pub const CONTAINER: &str = "nest_rs::container";
/// Guard, pipe, filter and interceptor chains as they compose.
pub const LAYERS: &str = "nest_rs::layers";
/// `#[hooks]` phases, in the order the app runs them.
pub const LIFECYCLE: &str = "nest_rs::lifecycle";
/// Module registration and imports.
pub const MODULE: &str = "nest_rs::module";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every target this crate declares.
    ///
    /// Test-local, and deliberately: the `filters` join reads the declarations
    /// out of the source rather than linking this crate, so a `pub` list has no
    /// reader outside these three checks — and a published array nobody calls is
    /// surface to keep in step for nothing.
    const ALL: [&str; 6] = [ACCESS_GRAPH, APP, CONTAINER, LAYERS, LIFECYCLE, MODULE];

    /// The shape `CLAUDE.md` fixes: `nest_rs::<concern>`, lowercase, **two**
    /// segments. A third would be a hierarchy this table does not have, and the
    /// prose has no way to notice one.
    #[test]
    fn every_target_is_two_lowercase_segments_rooted_at_the_framework() {
        for target in ALL {
            let concern = target
                .strip_prefix("nest_rs::")
                .unwrap_or_else(|| panic!("{target} is not rooted at the framework"));
            assert!(
                !concern.is_empty() && !concern.contains(':'),
                "{target} must be `nest_rs::<concern>` and nothing deeper",
            );
            assert!(
                concern.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{target} must be lowercase",
            );
        }
    }

    /// Two concerns under one name cannot be filtered apart, and the duplicate
    /// would read as a longer table rather than a shorter one.
    #[test]
    fn no_two_concerns_share_a_target() {
        let distinct: std::collections::BTreeSet<&str> = ALL.into_iter().collect();
        assert_eq!(distinct.len(), ALL.len(), "a target is declared twice");
    }

    /// `EnvFilter` matches a directive by `starts_with` on the raw string, so a
    /// target that prefixes another cannot be silenced alone — the defect
    /// `nest_rs::access` / `nest_rs::access_graph` shipped as. This crate's own
    /// six are checked here; the `filters` join in `nest-rs-conformance` checks
    /// the same property across every target both workspaces declare or spell,
    /// which is the population that can actually collide.
    #[test]
    fn no_target_is_a_prefix_of_another() {
        for outer in ALL {
            for inner in ALL {
                assert!(
                    outer == inner || !inner.starts_with(outer),
                    "a directive naming {outer} also selects {inner}",
                );
            }
        }
    }
}
