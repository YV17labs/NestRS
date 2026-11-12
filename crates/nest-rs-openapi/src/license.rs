//! [`OpenApiLicense`] — the API's license, the document's `info.license`.

/// The license the API is offered under, OpenAPI's License Object: set through
/// `<PREFIX>_OPENAPI__LICENSE_*` or pinned on [`OpenApiConfig`](crate::OpenApiConfig).
///
/// `identifier` (an SPDX expression) and `url` are mutually exclusive
/// (OpenAPI 3.1 §4.8.4); a config naming both is refused at boot.
///
/// ```
/// use nest_rs_openapi::OpenApiLicense;
///
/// let license = OpenApiLicense::new("Apache 2.0").with_identifier("Apache-2.0");
/// assert_eq!(license.identifier.as_deref(), Some("Apache-2.0"));
/// assert_eq!(license.url, None);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenApiLicense {
    /// The license's name, e.g. `Apache 2.0`.
    pub name: String,
    /// An SPDX license expression, e.g. `Apache-2.0`.
    pub identifier: Option<String>,
    /// A URL to the license text.
    pub url: Option<String>,
}

impl OpenApiLicense {
    /// The license `name`, OpenAPI's one required field.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            identifier: None,
            url: None,
        }
    }

    /// The license as an SPDX expression names it, e.g. `Apache-2.0`.
    pub fn with_identifier(mut self, identifier: impl Into<String>) -> Self {
        self.identifier = Some(identifier.into());
        self
    }

    /// The license as the text at `url` states it.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}
