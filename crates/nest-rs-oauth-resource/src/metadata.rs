//! [`ProtectedResourceMetadata`] — the RFC 9728 document this deployment
//! serves, and the `WWW-Authenticate` challenge that points at it.
//!
//! Built once at boot from [`OAuthResourceConfig`](super::OAuthResourceConfig)
//! and provided as global infrastructure, so the well-known controller and the
//! challenge interceptor read the same frozen value — the document a client
//! fetches can never disagree with the challenge that sent it there.

use nest_rs_http::challenge::BEARER;
use serde::Serialize;

/// The well-known path RFC 9728 §3 reserves, served at the resource's root.
pub const WELL_KNOWN_PATH: &str = "/.well-known/oauth-protected-resource";

/// The four RFC 9728 §2 members that describe the resource to a human rather
/// than to a client's protocol logic. Grouped because they travel together and
/// are all OPTIONAL `String`s under the same §2.1 internationalization rules —
/// four more positional arguments would have said less.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResourceDescription {
    pub resource_name: Option<String>,
    pub resource_documentation: Option<String>,
    pub resource_policy_uri: Option<String>,
    pub resource_tos_uri: Option<String>,
}

/// OAuth 2.0 Protected Resource Metadata (RFC 9728 §2). Every optional member
/// is omitted rather than serialized empty, as §3.2 requires: a client reads
/// absence as "not advertised" and an empty array as "supports nothing".
///
/// **§2 defines fourteen members; this ships eight, and the six it does not are
/// refused rather than left unsaid** — each names a fact a reader can check:
///
/// - `jwks_uri` and `resource_signing_alg_values_supported` describe a resource
///   that *signs its own responses*. Nothing in this framework signs a
///   response, so there is no key to publish and no algorithm to name.
/// - `dpop_signing_alg_values_supported` and `dpop_bound_access_tokens_required`
///   describe RFC 9449 sender-constrained tokens. `JwtService` verifies a
///   bearer token and has no `cnf` claim handling, so advertising either would
///   promise a binding nothing checks.
/// - `tls_client_certificate_bound_access_tokens` is RFC 8705's mTLS binding,
///   which needs the peer certificate from the TLS layer; the framework's
///   `Strategy` sees a `poem::Request` and never the handshake.
/// - `signed_metadata` (§3.3) obliges a *recipient* to validate a JWT-wrapped
///   copy of this document. Publishing one is possible; doing it without the
///   validating half is half a feature, so it is an owner question rather than
///   a refusal.
#[derive(Clone, Debug, Serialize)]
pub struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scopes_supported: Vec<String>,
    bearer_methods_supported: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_policy_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_tos_uri: Option<String>,
    /// Absolute URL of this document, derived from `resource` per RFC 9728
    /// §3.1. Not an RFC 9728 field — it is what the challenge advertises, kept
    /// here so the two cannot drift.
    #[serde(skip)]
    metadata_url: String,
    /// The resource's own path component, without its leading slash (`mcp` for
    /// `https://api.example.com/mcp`, empty when the resource is a bare
    /// origin). The path-aware route compares against this so the document is
    /// served for *this* resource and nothing else.
    #[serde(skip)]
    resource_path: String,
    /// Pre-rendered `WWW-Authenticate` value, built once rather than per 401.
    #[serde(skip)]
    challenge: String,
}

impl ProtectedResourceMetadata {
    /// Freeze a validated configuration into the served document. Private to
    /// the crate: every field has already been checked by
    /// [`OAuthResourceConfig::into_metadata`](super::OAuthResourceConfig::into_metadata),
    /// and constructing one by hand would bypass those checks.
    pub(crate) fn new(
        resource: String,
        authorization_servers: Vec<String>,
        scopes_supported: Vec<String>,
        bearer_methods_supported: Vec<String>,
        description: ResourceDescription,
    ) -> Self {
        let ResourceDescription {
            resource_name,
            resource_documentation,
            resource_policy_uri,
            resource_tos_uri,
        } = description;
        let (origin, path) = split_resource(&resource);
        // RFC 9728 §3.1: the well-known string goes *between* the authority and
        // the resource's path, so a resource at `https://host/mcp` publishes at
        // `https://host/.well-known/oauth-protected-resource/mcp`. Hanging the
        // document off the origin instead would answer for a resource this
        // deployment may not be.
        let metadata_url = format!("{origin}{WELL_KNOWN_PATH}{path}");
        // The *path* the route serves, which is the identifier's path component and
        // nothing else: a URL path never carries the query, so keeping it here made
        // the deployment advertise a `metadata_url` its own route answered `404`
        // for — a conformant client following the challenge dead-ended while one
        // that guessed the origin form succeeded.
        let resource_path = path
            .split('?')
            .next()
            .unwrap_or(path)
            .trim_start_matches('/')
            .to_owned();
        let challenge = build_challenge(&metadata_url, &scopes_supported, None);
        Self {
            resource,
            authorization_servers,
            resource_policy_uri,
            resource_tos_uri,
            scopes_supported,
            bearer_methods_supported,
            resource_name,
            resource_documentation,
            metadata_url,
            resource_path,
            challenge,
        }
    }

    /// The canonical URI clients name in their RFC 8707 `resource` parameter —
    /// and the `aud` every accepted token must carry.
    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// Issuers a client may obtain a token from.
    pub fn authorization_servers(&self) -> &[String] {
        &self.authorization_servers
    }

    /// The advertised minimal scope set; empty when the deployment names none.
    pub fn scopes_supported(&self) -> &[String] {
        &self.scopes_supported
    }

    /// How a token may be presented (RFC 9728 §2).
    pub fn bearer_methods_supported(&self) -> &[String] {
        &self.bearer_methods_supported
    }

    /// Absolute URL of this document — what the challenge's `resource_metadata`
    /// points at.
    pub fn metadata_url(&self) -> &str {
        &self.metadata_url
    }

    /// The resource's path component without its leading slash, empty for a
    /// bare origin. The path-aware well-known route matches its tail against
    /// this.
    pub(crate) fn resource_path(&self) -> &str {
        &self.resource_path
    }

    /// The `WWW-Authenticate` value for a `401`: `Bearer` plus
    /// `resource_metadata`, and `scope` when the deployment advertises one.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    /// The `WWW-Authenticate` value for a `403` whose token is valid but too
    /// narrow (RFC 6750 §3.1). Built per call because the required scope is a
    /// property of the operation, not of the deployment.
    pub fn insufficient_scope_challenge(&self, required: &[String]) -> String {
        build_challenge(
            &self.metadata_url,
            if required.is_empty() {
                &self.scopes_supported
            } else {
                required
            },
            Some("insufficient_scope"),
        )
    }
}

/// Split a canonical resource URI into its `scheme://authority` origin and its
/// path (`""` or `/…`, trailing slash trimmed) — the two halves RFC 9728 §3.1
/// puts on either side of the well-known string.
fn split_resource(resource: &str) -> (&str, &str) {
    let Some((scheme, rest)) = resource.split_once("://") else {
        // Unreachable via `into_metadata`, which refuses a scheme-less URI.
        // Falling back to the whole string keeps this total rather than
        // panicking on a value a future caller might construct.
        return (resource, "");
    };
    // RFC 9728 §3.1 names **both**: "If the resource identifier value contains
    // a path or query component, any terminating slash (/) following the host
    // component MUST be removed before inserting /.well-known/ and the
    // well-known URI path suffix between the host component and the path and/or
    // query components." Splitting on `/` alone put the well-known suffix
    // *inside* the query string of a query-only identifier, which §1.2 admits
    // as a real case ("it is recognized that there are cases that make a query
    // component a useful and necessary part of a resource identifier").
    match rest.find(['/', '?']) {
        Some(at) => {
            let split = scheme.len() + "://".len() + at;
            // A resource written with a trailing slash means the same resource;
            // carrying the slash into the well-known URL would publish at
            // `…/oauth-protected-resource/` and miss the client's request.
            (&resource[..split], resource[split..].trim_end_matches('/'))
        }
        None => (resource, ""),
    }
}

/// Assemble a `WWW-Authenticate: Bearer …` value. Parameter order follows the
/// examples in the MCP authorization spec, which lead with `error` when there
/// is one.
fn build_challenge(metadata_url: &str, scopes: &[String], error: Option<&str>) -> String {
    // Collected then joined rather than pushed with hand-placed separators: the
    // `", "` belonged to two different arms, so adding a parameter meant getting
    // the comma right in both.
    let mut params = Vec::with_capacity(3);
    if let Some(error) = error {
        params.push(format!("error=\"{error}\""));
    }
    params.push(format!("resource_metadata=\"{metadata_url}\""));
    if !scopes.is_empty() {
        params.push(format!("scope=\"{}\"", scopes.join(" ")));
    }
    format!("{BEARER} {}", params.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(resource: &str, scopes: &[&str]) -> ProtectedResourceMetadata {
        ProtectedResourceMetadata::new(
            resource.to_owned(),
            vec!["https://auth.example.com".into()],
            scopes.iter().map(|s| (*s).to_owned()).collect(),
            vec!["header".into()],
            Default::default(),
        )
    }

    #[test]
    fn the_well_known_string_goes_between_the_authority_and_the_path() {
        // RFC 9728 §3.1 — *not* `https://api.example.com/mcp/.well-known/…`
        // (which would nest it under the resource), and not the bare origin
        // (which would answer for a resource this deployment may not be).
        assert_eq!(
            meta("https://api.example.com/mcp", &[]).metadata_url(),
            "https://api.example.com/.well-known/oauth-protected-resource/mcp",
        );
        assert_eq!(
            meta("https://api.example.com:8443", &[]).metadata_url(),
            "https://api.example.com:8443/.well-known/oauth-protected-resource",
        );
    }

    #[test]
    fn a_bare_origin_publishes_at_the_unsuffixed_path() {
        let meta = meta("https://api.example.com", &[]);
        assert_eq!(
            meta.metadata_url(),
            "https://api.example.com/.well-known/oauth-protected-resource",
        );
        assert_eq!(
            meta.resource_path(),
            "",
            "nothing for the tail route to match"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_leak_into_the_published_url() {
        // `https://api.example.com/mcp/` names the same resource; publishing at
        // `…/oauth-protected-resource/mcp/` would miss the client's request for
        // `…/mcp`.
        let meta = meta("https://api.example.com/mcp/", &[]);
        assert_eq!(
            meta.metadata_url(),
            "https://api.example.com/.well-known/oauth-protected-resource/mcp",
        );
        assert_eq!(meta.resource_path(), "mcp");
    }

    #[test]
    fn a_multi_segment_resource_path_survives_whole() {
        let meta = meta("https://api.example.com/v1/mcp", &[]);
        assert_eq!(
            meta.metadata_url(),
            "https://api.example.com/.well-known/oauth-protected-resource/v1/mcp",
        );
        assert_eq!(meta.resource_path(), "v1/mcp");
    }

    #[test]
    fn the_challenge_carries_the_metadata_url() {
        assert_eq!(
            meta("https://api.example.com", &[]).challenge(),
            "Bearer resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\"",
        );
    }

    #[test]
    fn advertised_scopes_ride_the_challenge() {
        assert_eq!(
            meta("https://api.example.com", &["posts:read", "posts:write"]).challenge(),
            "Bearer resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\", \
             scope=\"posts:read posts:write\"",
        );
    }

    #[test]
    fn an_insufficient_scope_challenge_names_the_error_and_the_missing_scope() {
        let challenge = meta("https://api.example.com", &["posts:read"])
            .insufficient_scope_challenge(&["posts:write".to_owned()]);
        assert_eq!(
            challenge,
            "Bearer error=\"insufficient_scope\", \
             resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\", \
             scope=\"posts:write\"",
        );
    }

    #[test]
    fn empty_optional_fields_are_omitted_from_the_document() {
        // An empty `scopes_supported` array would tell a client this resource
        // supports no scopes at all; absence says "not advertised".
        let json = serde_json::to_value(meta("https://api.example.com", &[])).expect("serializes");
        assert!(json.get("scopes_supported").is_none());
        assert!(json.get("resource_name").is_none());
        assert_eq!(json["resource"], "https://api.example.com");
        assert_eq!(json["authorization_servers"][0], "https://auth.example.com");
        assert_eq!(json["bearer_methods_supported"][0], "header");
    }

    /// RFC 9728 §3.1 names a **query** component beside a path: "If the
    /// resource identifier value contains a path or query component, any
    /// terminating slash (/) following the host component MUST be removed
    /// before inserting /.well-known/ … between the host component and the
    /// path and/or query components." §1.2 admits the case explicitly. Splitting
    /// on `/` alone put the well-known suffix *inside* the query string.
    #[test]
    fn a_query_component_is_split_like_a_path() {
        assert_eq!(
            split_resource("https://api.example.com?tenant=a"),
            ("https://api.example.com", "?tenant=a"),
        );
        assert_eq!(
            meta("https://api.example.com?tenant=a", &[]).metadata_url(),
            "https://api.example.com/.well-known/oauth-protected-resource?tenant=a",
        );
        // Path and query together keep both, in order.
        assert_eq!(
            split_resource("https://api.example.com/mcp?tenant=a"),
            ("https://api.example.com", "/mcp?tenant=a"),
        );
    }

    #[test]
    fn the_document_never_leaks_the_internal_challenge_fields() {
        let json = serde_json::to_value(meta("https://api.example.com", &["a"])).expect("json");
        let object = json.as_object().expect("an object");
        assert!(!object.contains_key("challenge"));
        assert!(!object.contains_key("metadata_url"));
    }
}
