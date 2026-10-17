//! The span targets this crate owns.

/// The ORM: every query, transaction and row-level filter applied.
pub const ORM: &str = "nest_rs::orm";
/// Relation dataloaders `#[expose(…, graphql)]` emits, and their batches.
pub const LOADER: &str = "nest_rs::loader";

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 2] = [ORM, LOADER];

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
