//! [`AppIdentity`] — the binary's name, version and description, read off its
//! package's manifest where `#[nest_rs::main]` expands: what every surface
//! naming the app defaults to, the boot's first line, OpenAPI's `info`, MCP's
//! `serverInfo` and OpenTelemetry's `service.name` and `service.version`.

use std::sync::OnceLock;

/// The binary's name, version and description, as its package's manifest
/// declares them (`CARGO_PKG_NAME`, `CARGO_PKG_VERSION`,
/// `CARGO_PKG_DESCRIPTION`), captured by `#[nest_rs::main]` before the runtime
/// exists. Each surface naming the app defaults to it and keeps its own
/// override.
///
/// ```
/// use nest_rs_core::AppIdentity;
///
/// // No `#[nest_rs::main]` started this process.
/// assert!(AppIdentity::current().is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppIdentity {
    /// The package's name.
    pub name: String,
    /// The package's version.
    pub version: String,
    /// The package's one-line description; `None` when its manifest has none.
    pub description: Option<String>,
}

/// The identity this process captured, once.
static CAPTURED: OnceLock<AppIdentity> = OnceLock::new();

impl AppIdentity {
    /// The identity this process captured — its `#[nest_rs::main]`'s, or the
    /// package a test harness runs for — or `None` when neither started it.
    pub fn current() -> Option<&'static AppIdentity> {
        CAPTURED.get()
    }
}

/// The identity a package's manifest declares; its description is `None` when
/// empty, as Cargo leaves an undeclared one.
pub fn package_identity(name: &str, version: &str, description: &str) -> AppIdentity {
    AppIdentity {
        name: name.to_owned(),
        version: version.to_owned(),
        description: Some(description.trim())
            .filter(|description| !description.is_empty())
            .map(str::to_owned),
    }
}

/// Record `identity` for the process. A process runs one `main`, so the first
/// recording stands.
pub fn capture_app_identity(identity: AppIdentity) {
    CAPTURED.get_or_init(|| identity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_description_is_none_and_a_declared_one_is_kept() {
        assert_eq!(package_identity("api", "7.0.0", "").description, None);
        assert_eq!(package_identity("api", "7.0.0", "  ").description, None);
        let identity = package_identity("api", "7.0.0", "The demo's API");
        assert_eq!(identity.name, "api");
        assert_eq!(identity.version, "7.0.0");
        assert_eq!(identity.description.as_deref(), Some("The demo's API"));
    }
}
