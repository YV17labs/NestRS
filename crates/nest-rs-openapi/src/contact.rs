//! [`OpenApiContact`] — who answers for the API, the document's `info.contact`.

/// The contact the document publishes for the API, OpenAPI's Contact Object:
/// set through `<PREFIX>_OPENAPI__CONTACT_*` or pinned on
/// [`OpenApiConfig`](crate::OpenApiConfig).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenApiContact {
    /// The person or team's name.
    pub name: Option<String>,
    /// A URL pointing at the contact information.
    pub url: Option<String>,
    /// An email address.
    pub email: Option<String>,
}
