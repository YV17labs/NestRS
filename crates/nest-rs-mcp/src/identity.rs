//! [`McpEndpoint`] — the identity of one MCP endpoint, declared by the app.
//!
//! # Why the app declares it, and not a host
//!
//! An MCP endpoint is **one server** to every client that speaks to it: the
//! protocol carries exactly one `serverInfo` and one `instructions` per
//! endpoint, on `initialize` and on `server/discover` alike. That is not a
//! nestrs opinion — it is what the spec's `DiscoverResult` holds, what the
//! TypeScript SDK constructs (`new McpServer({ name, version })`, tools
//! registered onto it), and what FastMCP means when a parent server keeps its
//! own name and mounts children into it.
//!
//! So identity belongs to whoever owns the *endpoint*. A `#[mcp]` host owns a
//! **feature**, and several of them share a path — asking one to speak for the
//! whole endpoint would make the endpoint's name a function of `imports = [..]`
//! order, which is exactly the accident this type removes.
//!
//! # What it replaces, and what it never touches
//!
//! Declared identity replaces **what it states**: `serverInfo` always (name and
//! version are required), `instructions` when it declares them. Everything else
//! stays a fact about the hosts and is still merged from them — capabilities are
//! the union of what they serve, protocol versions the intersection of what they
//! implement. A declaration cannot claim a capability no host provides.
//!
//! Undeclared, a lone host is its own endpoint (the ordinary shape, and the norm
//! at N=1) and a shared path falls back to the first host's identity with a boot
//! `warn` naming it.
//!
//! ```no_run
//! use nest_rs_mcp::{McpEndpoint, McpModule};
//! use nest_rs_core::module;
//!
//! #[module(imports = [
//!     McpModule::endpoint(
//!         McpEndpoint::new("/mcp", "assistant", env!("CARGO_PKG_VERSION"))
//!             .instructions("Search posts and people. Ask before writing."),
//!     ),
//! ])]
//! struct AppModule;
//! ```

use std::borrow::Cow;

use rmcp::model::{Icon, Implementation};

/// The identity an app declares for the MCP endpoint at one path.
///
/// Declared through [`McpModule::endpoint`](crate::McpModule::endpoint); read
/// back with [`declared_endpoint`](crate::declared_endpoint). Declaring a path
/// no `#[mcp]` host serves fails boot — a declaration that reaches nothing is a
/// typo, not a no-op.
#[derive(Clone, Debug, PartialEq)]
pub struct McpEndpoint {
    path: Cow<'static, str>,
    info: Implementation,
    instructions: Option<String>,
}

impl McpEndpoint {
    /// Declare the endpoint at `path` as `name` version `version`.
    ///
    /// Both are mandatory because the protocol's `serverInfo` carries both, and
    /// a version defaulted from the *framework's* build environment would name
    /// nest-rs where the deployment meant its own app. `env!("CARGO_PKG_VERSION")`
    /// at the call site is the app's.
    pub fn new(
        path: impl Into<Cow<'static, str>>,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            info: Implementation::new(name, version),
            instructions: None,
        }
    }

    /// Display name for a human-facing client list, when it should differ from
    /// the machine `name`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.info.title = Some(title.into());
        self
    }

    /// One line about what the endpoint is, for a client's server list.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.info.description = Some(description.into());
        self
    }

    /// Homepage or documentation URL a client can offer the user.
    pub fn website_url(mut self, url: impl Into<String>) -> Self {
        self.info.website_url = Some(url.into());
        self
    }

    /// Icons a client may show beside the endpoint.
    pub fn icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.info.icons = Some(icons.into_iter().collect());
        self
    }

    /// Natural-language guidance the model reads before using the endpoint.
    ///
    /// This is the one part of the declaration a model actually acts on, and the
    /// reason the endpoint — not a feature — writes it: it frames *the whole
    /// surface*, which no single host can see. Declared, it replaces the hosts'
    /// own; left out, the hosts' are joined instead of dropped.
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// The endpoint path this identity is declared for.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The `serverInfo` the endpoint reports.
    pub fn implementation(&self) -> &Implementation {
        &self.info
    }

    /// The declared instructions, `None` when the endpoint leaves them to its
    /// hosts.
    pub fn declared_instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }
}
