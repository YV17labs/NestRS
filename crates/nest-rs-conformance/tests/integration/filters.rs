//! No declared `tracing` target is a raw-string prefix of another.
//!
//! An operator's only handle on a target is a filter directive, and
//! `EnvFilter` matches one with `starts_with` on the **raw string** rather than
//! on `::` segments (`tracing-subscriber`'s `Directive::cares_about`). So a
//! target that is a prefix of another is a directive that cannot address the
//! first without also addressing the second, and the operator gets no sign that
//! it did. That shipped once: `nest_rs::access` swallowed
//! `nest_rs::access_graph`, and quieting the per-unit line cost a startup
//! diagnostic.
//!
//! The population is the target **constants** the framework declares — a
//! crate's root `TARGET`, or a `mod target` of them — read by
//! [`declared_targets`]. A call site naming its target as a literal is outside
//! it, and that is a review item: the rule makes a target a constant its owner
//! declares, so a literal is already the defect.

use nest_rs_conformance::sources::declared_targets;

/// The framework declares well above this many; below it the walk is reading
/// the wrong tree.
const FLOOR: usize = 20;

/// Whether a directive naming `outer` also selects `inner` — `EnvFilter`'s own
/// predicate, not a segment-aware approximation of it. Equal strings are one
/// target, not a pair.
fn swallows(outer: &str, inner: &str) -> bool {
    outer != inner && inner.starts_with(outer)
}

#[test]
fn no_target_is_a_prefix_of_another() {
    let declared = declared_targets();
    assert!(
        declared.len() >= FLOOR,
        "read {} declared target(s), expected at least {FLOOR}",
        declared.len(),
    );

    let mut pairs: Vec<String> = Vec::new();
    for (outer, outer_crate, outer_const) in declared {
        for (inner, inner_crate, inner_const) in declared {
            if swallows(outer, inner) {
                pairs.push(format!(
                    "`{outer}` ({outer_crate}::{outer_const}) swallows \
                     `{inner}` ({inner_crate}::{inner_const})"
                ));
            }
        }
    }
    pairs.sort();
    assert!(
        pairs.is_empty(),
        "{} declared target(s) cannot be named by a filter directive without \
         naming another, because `EnvFilter` matches by `starts_with` on the raw \
         string. Rename one side of each pair:\n  {}",
        pairs.len(),
        pairs.join("\n  "),
    );
}
