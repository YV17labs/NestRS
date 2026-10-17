//! [`McpIdentity`] — who an MCP endpoint says it is.
//!
//! An endpoint reports one `serverInfo` and one `instructions`. The app declares
//! them through [`McpOptions::server`](crate::McpOptions::server); a host may
//! refine only `name` and `title` for its endpoint, and two hosts declaring on one
//! path fail the boot. Capabilities are never declared, only observed from the hosts.

use rmcp::model::{Icon, Implementation};

/// What an app, or one `#[mcp]` host, says about the server behind an endpoint:
/// [`new`](Self::new) for the app's own `serverInfo`, the `#[mcp]` attribute for a
/// host's refinement of the fields it writes.
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

    /// How to use this server, which a client may fold into its system prompt;
    /// what each tool does belongs to its own `#[tool(description = "…")]`.
    ///
    /// Declared, it replaces whatever the hosts wrote through
    /// `ServerHandler::get_info`; left out, theirs are joined.
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Whether this declaration states anything at all; an argumentless `#[mcp]`
    /// host is no claimant on its path.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One endpoint's identity, after a host's declaration has been laid over the
/// app's.
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

/// Lay `host`'s declaration over the app's `server`, field by field; a name with
/// no version behind it is refused, never reported at the SDK's version.
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
             version — an endpoint reports both, and the version is the app's to declare, \
             since a feature library knows neither the binary's version nor, on a shared \
             endpoint, the whole surface. Name the app once with \
             McpModule::for_root(McpOptions {{ server: \
             Some(McpIdentity::new(\"{name}\", env!(\"CARGO_PKG_VERSION\"))), ..Default::default() \
             }})",
        )
    }
}

/// What the `#[mcp]` attribute declares; the version arrives from the app
/// alone, through [`McpIdentity::new`].
pub fn declared_identity(name: Option<&str>, title: Option<&str>) -> McpIdentity {
    McpIdentity {
        name: name.map(Into::into),
        title: title.map(Into::into),
        ..McpIdentity::default()
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

    #[test]
    fn a_host_overrides_per_field_and_inherits_the_rest() {
        let host = declared_identity(Some("assistant-posts"), None);
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
    fn a_host_declaring_both_of_its_fields_still_inherits_the_version() {
        let host = declared_identity(Some("assistant-posts"), Some("Posts"));
        let resolved = resolve(Some(&host), Some(&app())).expect("no error");
        let info = resolved.implementation().expect("named");

        assert_eq!(info.name, "assistant-posts");
        assert_eq!(
            info.title.as_deref(),
            Some("Posts"),
            "the host's title wins"
        );
        assert_eq!(
            info.version, "1.2.3",
            "the version has one owner and it is not the host",
        );
    }

    #[test]
    fn nobody_naming_the_server_resolves_to_nothing_rather_than_a_guess() {
        let resolved = resolve(None, None).expect("no error");
        assert!(!resolved.is_declared());
        assert!(resolved.implementation().is_none());
    }

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
        let host = declared_identity(Some("orphan"), None);
        let err = resolve(Some(&host), None).expect_err("rejected");
        let message = err.report("/mcp/posts", "PostsTool");

        assert!(message.contains("PostsTool"), "names the host: {message}");
        assert!(message.contains("/mcp/posts"), "names the path: {message}");
        assert!(
            message.contains("McpModule::for_root"),
            "carries the remedy: {message}",
        );
        assert!(
            !message.contains("#[mcp]"),
            "…and only the remedy that exists — a host has no `version` argument \
             to reach for: {message}",
        );
    }

    #[test]
    fn an_argumentless_mcp_host_declares_nothing() {
        assert!(declared_identity(None, None).is_empty());
        assert!(!McpIdentity::new("a", "1").is_empty());
    }

    /// A field added to [`McpIdentity`] fails to compile here until listed; rustc
    /// words it "inaccessible fields" inside a macro — list it, never write `..`.
    macro_rules! every_field {
        ($($field:ident),+ $(,)?) => {{
            let McpIdentity { $($field: _),+ } = McpIdentity::default();
            [$(stringify!($field)),+]
        }};
    }

    /// Each field owes `#[mcp(<field> = …)]` an answer, never the bare unknown-key sentence.
    #[test]
    fn every_identity_field_has_an_answer_at_the_host() {
        let fields = every_field!(
            name,
            version,
            title,
            description,
            website_url,
            icons,
            instructions,
        );
        for field in fields {
            assert!(
                nest_rs_codegen::mcp_answers(field),
                "`McpIdentity::{field}` has no answer in `#[mcp]`'s grammar — give it a \
                 row in nest_rs_codegen's SERVER_FIELDS naming the seam that takes it",
            );
        }
    }
}
