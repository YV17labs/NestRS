//! [`OpenApiLicense`] — the API's license, the document's `info.license`.

/// The license the API is offered under, OpenAPI's License Object: set through
/// `<PREFIX>_OPENAPI__LICENSE_*` or pinned on [`OpenApiConfig`](crate::OpenApiConfig).
///
/// `identifier` (an SPDX expression) and `url` are mutually exclusive
/// (OpenAPI 3.1 §4.8.4); a config naming both is refused at boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenApiLicense {
    /// The license's name, e.g. `Apache 2.0`.
    pub name: String,
    /// An SPDX license expression, e.g. `Apache-2.0`.
    pub identifier: Option<String>,
    /// A URL to the license text.
    pub url: Option<String>,
}
