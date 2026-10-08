//! The span targets of the concerns **this crate** owns, read by every crate
//! that emits on them. The operation log's is
//! [`operation_log::TARGET`](crate::operation_log::TARGET).

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

    const ALL: [&str; 5] = [APP, CONTAINER, LAYERS, LIFECYCLE, MODULE];

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

    #[test]
    fn no_two_concerns_share_a_target() {
        let distinct: std::collections::BTreeSet<&str> = ALL.into_iter().collect();
        assert_eq!(distinct.len(), ALL.len(), "a target is declared twice");
    }

    /// `EnvFilter` matches a directive by `starts_with` on the raw string.
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
