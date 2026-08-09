//! [`McpIdentity`] — who an MCP endpoint says it is.
//!
//! # Two declarations, two owners, no overlap
//!
//! An MCP endpoint is **one server** to every client that speaks to it: the
//! protocol carries exactly one `serverInfo` and one `instructions` per
//! endpoint, on `initialize` and on `server/discover` alike. That is not a
//! nestrs opinion — it is what the spec's `DiscoverResult` holds, what the
//! TypeScript SDK constructs (`new McpServer({ name, version })`, tools
//! registered onto it), and what FastMCP means when a parent server keeps its
//! own name and mounts children into it.
//!
//! So one endpoint answers with one identity, and the two halves of it are owned
//! by whoever can actually know them:
//!
//! * **The app says who it is, and how its server is used** — `name`, `version`,
//!   the branding a client shows beside them, and the `instructions` a client
//!   may fold into the model's system prompt. All of it travels once, through
//!   [`McpOptions::server`](crate::McpOptions::server). The version has to come
//!   from there: `env!("CARGO_PKG_VERSION")` written in a shared feature library
//!   is the library's version, not the deployment's. So do the instructions:
//!   they describe **the server**, not a feature, and on a shared endpoint no
//!   single host can see the whole.
//! * **A host says which endpoint stands apart** — a `name`/`title` on `#[mcp]`,
//!   when one endpoint should not be reported as the app's default. That is a
//!   fact about the mount, so it is declared where the mount is.
//!
//! Neither can shadow the other by accident: a host's fields override the app's
//! per field, and two hosts sharing one path may not both declare — that fails
//! boot naming them, because the answer would otherwise depend on
//! `imports = [..]` order.
//!
//! **Per-tool prose is a different field entirely.** `instructions` is the
//! server's — how to use it at all — and `#[tool(description = "…")]` is each
//! tool's, which is what the model reads when it picks one. Writing tool
//! specifics into `instructions` duplicates a description that is already
//! carried where the model expects it.
//!
//! # What a declaration replaces, and what it never touches
//!
//! Identity is **declared**; capabilities are **observed**. A declaration
//! replaces `serverInfo`, and `instructions` when it writes any. Everything else
//! stays a fact about the hosts and is still merged from them — capabilities are
//! the union of what they serve, protocol versions the intersection of what they
//! implement. A declaration can never claim a capability no host provides.
//!
//! Undeclared instructions are **joined** from the hosts rather than dropped, so
//! a shared endpoint reads as the sum of its features until someone frames it.
//!
//! ```no_run
//! use nest_rs_core::module;
//! use nest_rs_mcp::{McpIdentity, McpModule, McpOptions};
//!
//! #[module(imports = [
//!     McpModule::for_root(McpOptions {
//!         server: Some(
//!             McpIdentity::new("assistant", env!("CARGO_PKG_VERSION"))
//!                 .instructions("Every result is scoped to the caller's token."),
//!         ),
//!         ..Default::default()
//!     }),
//! ])]
//! struct AppModule;
//! ```

use rmcp::model::{Icon, Implementation};

/// What an app, or one `#[mcp]` host, says about the server behind an endpoint.
///
/// Built two ways, and the constructor says which owner it came from:
/// [`new`](Self::new) for the app's own `serverInfo` (both halves mandatory —
/// the protocol carries both, and a version defaulted from the *framework's*
/// build environment would name nest-rs where the deployment meant its own app),
/// and the `#[mcp]` attribute for a host's per-endpoint refinement, which fills
/// only the fields it writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct McpIdentity {
    name: Option<String>,
    version: Option<String>,
    title: Option<String>,
    description: Option<String>,
    website_url: Option<String>,
    icons: Option<Vec<Icon>>,
    instructions: Option<String>,
}

impl McpIdentity {
    /// The app's own `serverInfo`: what every endpoint it exposes reports unless
    /// a host overrides it.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            version: Some(version.into()),
            ..Self::default()
        }
    }

    /// Display name for a human-facing client list, when it should differ from
    /// the machine `name`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// One line about what the server is, for a client's server list.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Homepage or documentation URL a client can offer the user.
    pub fn website_url(mut self, url: impl Into<String>) -> Self {
        self.website_url = Some(url.into());
        self
    }

    /// Icons a client may show beside the server.
    pub fn icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }

    /// How to use this server — the one part of the declaration a model acts
    /// on, which a client may fold into its system prompt.
    ///
    /// Declared here and nowhere else, because it describes **the server**: on
    /// an endpoint several features share, no single host can see the whole, and
    /// a paragraph assembled from the halves is one nobody wrote. Keep it
    /// general — what a caller must know to use the surface at all. What each
    /// tool *does* belongs to its own `#[tool(description = "…")]`, which is
    /// where the model reads it when choosing between them.
    ///
    /// Declared, it replaces whatever the hosts wrote through
    /// `ServerHandler::get_info`; left out, theirs are joined rather than
    /// dropped.
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// What the `#[mcp]` attribute declares — every field optional, because a
    /// host refines the app's identity rather than restating it.
    ///
    /// **Internal ABI** — named by the `#[mcp]` expansion, lockstep with this
    /// crate; never called by hand.
    #[doc(hidden)]
    pub fn declared(name: Option<&str>, version: Option<&str>, title: Option<&str>) -> Self {
        Self {
            name: name.map(Into::into),
            version: version.map(Into::into),
            title: title.map(Into::into),
            ..Self::default()
        }
    }

    /// Whether this declaration states anything at all — an `#[mcp]` host that
    /// wrote no identity argument declares nothing and must not count as a
    /// claimant on its path.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One endpoint's identity, after a host's declaration has been laid over the
/// app's. What the mount reports and what the boot checks judge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedIdentity {
    info: Option<Implementation>,
    instructions: Option<String>,
}

impl ResolvedIdentity {
    /// The `serverInfo` the endpoint reports, or `None` when nobody named it —
    /// in which case the endpoint falls back to its hosts (reported at boot by
    /// `registry::check_identity`).
    pub fn implementation(&self) -> Option<&Implementation> {
        self.info.as_ref()
    }

    /// The instructions declared for this endpoint. `None` leaves the hosts'
    /// own to be joined rather than replaced.
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// Whether anything was declared for this endpoint at all.
    pub fn is_declared(&self) -> bool {
        self.info.is_some() || self.instructions.is_some()
    }
}

/// Lay `host`'s declaration over the app's `server`, field by field.
///
/// The one way this can fail is a name with no version behind it: a host that
/// renames its endpoint in an app that never named itself. Reporting it is the
/// point — the alternative is an endpoint introducing itself at the *SDK's*
/// version, which reads as a working server and is not one.
pub(crate) fn resolve(
    host: Option<&McpIdentity>,
    app: Option<&McpIdentity>,
) -> Result<ResolvedIdentity, IdentityError> {
    let pick = |get: fn(&McpIdentity) -> Option<&str>| {
        host.and_then(get)
            .or_else(|| app.and_then(get))
            .map(str::to_owned)
    };

    let name = pick(|id| id.name.as_deref());
    let version = pick(|id| id.version.as_deref());
    let info = match (name, version) {
        (Some(name), Some(version)) => {
            let mut info = Implementation::new(name, version);
            info.title = pick(|id| id.title.as_deref());
            info.description = pick(|id| id.description.as_deref());
            info.website_url = pick(|id| id.website_url.as_deref());
            info.icons = host
                .and_then(|id| id.icons.clone())
                .or_else(|| app.and_then(|id| id.icons.clone()));
            Some(info)
        }
        (Some(name), None) => return Err(IdentityError::NameWithoutVersion(name)),
        (None, _) => None,
    };

    Ok(ResolvedIdentity {
        info,
        instructions: pick(|id| id.instructions.as_deref()),
    })
}

/// The one way [`resolve`] fails: a name nothing supplies a version for.
#[derive(Debug)]
pub(crate) enum IdentityError {
    NameWithoutVersion(String),
}

impl IdentityError {
    /// The boot message, with the remedy spelled out at the seam that fixes it.
    pub(crate) fn report(&self, path: &str, host: &str) -> String {
        let Self::NameWithoutVersion(name) = self;
        format!(
            "MCP host {host} names the endpoint at {path:?} {name:?} but nothing supplies a \
             version — an endpoint reports both. Either name the app once with \
             McpModule::for_root(McpOptions {{ server: \
             Some(McpIdentity::new(\"{name}\", env!(\"CARGO_PKG_VERSION\"))), ..Default::default() \
             }}), or write `version = ...` beside the name on #[mcp]",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> McpIdentity {
        McpIdentity::new("assistant", "1.2.3")
            .title("Assistant")
            .instructions("Every result is scoped to the caller's token.")
    }

    #[test]
    fn the_app_identity_is_what_an_undeclaring_host_reports() {
        let resolved = resolve(None, Some(&app())).expect("no error");
        let info = resolved.implementation().expect("named by the app");
        assert_eq!(info.name, "assistant");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.title.as_deref(), Some("Assistant"));
        assert_eq!(
            resolved.instructions(),
            Some("Every result is scoped to the caller's token."),
            "how to use the server is the app's word, for every endpoint it exposes",
        );
    }

    /// The point of the overlay: a host renames its own endpoint without having
    /// to restate — or be able to know — the app's version.
    #[test]
    fn a_host_overrides_per_field_and_inherits_the_rest() {
        let host = McpIdentity::declared(Some("assistant-posts"), None, None);
        let resolved = resolve(Some(&host), Some(&app())).expect("no error");
        let info = resolved.implementation().expect("named");

        assert_eq!(info.name, "assistant-posts", "the host's name wins");
        assert_eq!(info.version, "1.2.3", "the app's version is inherited");
        assert_eq!(
            info.title.as_deref(),
            Some("Assistant"),
            "a field the host left out keeps the app's",
        );
        assert_eq!(
            resolved.instructions(),
            Some("Every result is scoped to the caller's token."),
            "a host that stands apart still uses the server the same way — \
             instructions have one author and it is the app",
        );
    }

    #[test]
    fn nobody_naming_the_server_resolves_to_nothing_rather_than_a_guess() {
        let resolved = resolve(None, None).expect("no error");
        assert!(!resolved.is_declared());
        assert!(resolved.implementation().is_none());
    }

    /// An app may say how its server is used without renaming anything — the
    /// common case, since one app usually exposes one endpoint.
    #[test]
    fn instructions_alone_declare_without_naming() {
        let app = McpIdentity::default().instructions("Ask before writing.");
        assert!(!app.is_empty());
        let resolved = resolve(None, Some(&app)).expect("no error");
        assert!(resolved.is_declared());
        assert!(
            resolved.implementation().is_none(),
            "saying how to use the server is not naming it",
        );
        assert_eq!(resolved.instructions(), Some("Ask before writing."));
    }

    #[test]
    fn a_name_no_version_backs_is_a_boot_error_naming_the_remedy() {
        let host = McpIdentity::declared(Some("orphan"), None, None);
        let err = resolve(Some(&host), None).expect_err("rejected");
        let message = err.report("/mcp/posts", "PostsTool");

        assert!(message.contains("PostsTool"), "names the host: {message}");
        assert!(message.contains("/mcp/posts"), "names the path: {message}");
        assert!(
            message.contains("McpIdentity::new"),
            "carries the remedy: {message}",
        );
    }

    #[test]
    fn an_argumentless_mcp_host_declares_nothing() {
        assert!(McpIdentity::declared(None, None, None).is_empty());
        assert!(!McpIdentity::new("a", "1").is_empty());
    }
}
