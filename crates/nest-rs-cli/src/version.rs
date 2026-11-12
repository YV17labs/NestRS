//! The framework version a freshly scaffolded project pins — one source of
//! truth, derived so it can never drift.
//!
//! Derived from the CLI's own version: the workspace publishes in lockstep, so
//! the CLI's `major.minor` is the framework line it was cut from.

/// The semver requirement (`"<major>.<minor>"`) generated manifests pin for
/// every `nest-rs-*` crate. Tracks the CLI's own `CARGO_PKG_VERSION`, so a
/// lockstep release moves it with zero manual edits.
pub(crate) fn framework_req() -> String {
    let mut parts = env!("CARGO_PKG_VERSION").split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    format!("{major}.{minor}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_is_major_minor_of_the_cli() {
        let want: String = env!("CARGO_PKG_VERSION")
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".");
        assert_eq!(framework_req(), want);
    }

    #[test]
    fn req_drops_the_patch_component() {
        // A `"0.2"` requirement (= `^0.2`) accepts every 0.2.x patch; pinning
        // the patch would reject the very fixes a lockstep release ships.
        assert_eq!(framework_req().split('.').count(), 2);
    }
}
