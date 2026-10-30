//! The identity of the package a test process runs for, published as
//! `#[nest_rs::main]` publishes the binary's, so a suite boots its app under
//! the name and version the binary would report.

use nest_rs_core::__private::{capture_app_identity, package_identity};

/// Publish the package's identity from what `cargo test` and `cargo nextest`
/// hand a test process: its manifest's name, version and description. A
/// process neither started, or one a `#[nest_rs::main]` already named, is
/// left as it is.
#[expect(
    clippy::disallowed_methods,
    reason = "Cargo's own variables, set on the test process, which no prefix renames"
)]
pub(crate) fn publish_package_identity() {
    let read = |name: &str| std::env::var(name).ok();
    if let (Some(name), Some(version)) = (read("CARGO_PKG_NAME"), read("CARGO_PKG_VERSION")) {
        let description = read("CARGO_PKG_DESCRIPTION").unwrap_or_default();
        capture_app_identity(package_identity(&name, &version, &description));
    }
}
