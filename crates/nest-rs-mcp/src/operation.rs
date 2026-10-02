//! What one MCP operation *is*, for the layers that run around it.
//!
//! An HTTP handler is handed a `Request` and a mounted `RouteShaper`; a GraphQL
//! resolver is handed a `Context` carrying the [`Container`]. An MCP operation
//! is handed neither: rmcp builds the host once per session and dispatches each
//! operation on its own spawned task, so a generated prelude has nothing to read
//! the app off. This module is that seam.
//!
//! [`McpOperationContext`] is what a [`Guard`] then sees. It deliberately does
//! **not** carry the operation's arguments: deciding *access* from a payload is
//! a pipe's job (`Valid<T>` / `Piped<P, T>` run on the wire value before the
//! body), and handing a guard the arguments invites the check to migrate into
//! the one place the layer rules say it must not be.

use std::fmt;

use nest_rs_core::Container;

/// The container serving the current MCP operation, if one is installed.
///
/// Read off the ambient request scope rather than carried again: the HTTP
/// transport edge builds that scope with the app container and the MCP endpoint
/// nests under it, so a second copy would be one more thing two installs have to
/// agree about. `None` outside an MCP dispatch, and inside one whose mount is
/// not nested under the transport edge.
pub fn current_container() -> Option<Container> {
    crate::scope::current_scope().map(|scope| scope.root().clone())
}

/// Which router an operation belongs to — the two roles `#[tools]` serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpOperationKind {
    /// A `#[tool]` method, reached by `tools/call`.
    Tool,
    /// A `#[prompt]` method, reached by `prompts/get`.
    Prompt,
}

impl McpOperationKind {
    /// The lowercase word this role is logged and labelled under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Prompt => "prompt",
        }
    }
}

impl fmt::Display for McpOperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One MCP operation, as a [`Guard`](https://docs.rs/nest-rs-guards) sees it —
/// the MCP analog of the `&Context` a `check_graphql` takes and the
/// `(client, event, data)` a `check_ws_message` takes.
///
/// Built by the `#[tools]` expansion around each decorated operation. The caller's
/// `Ability` is **ambient** by the time a guard runs (the endpoint's
/// [`McpOperationGuard`](crate::McpOperationGuard) installs it in its `around`),
/// so a capability-only guard reads it with `nest_rs_authz::current_ability`
/// exactly as it would on any other transport.
pub struct McpOperationContext<'a> {
    container: &'a Container,
    host: &'static str,
    kind: McpOperationKind,
    name: &'static str,
}

impl<'a> McpOperationContext<'a> {
    /// Describe the operation about to run. Macro-emitted; the arguments come
    /// from the decorated method, so every field is `'static` but the container.
    pub fn new(
        container: &'a Container,
        host: &'static str,
        kind: McpOperationKind,
        name: &'static str,
    ) -> Self {
        Self {
            container,
            host,
            kind,
            name,
        }
    }

    /// The app serving this operation — what a guard resolves collaborators
    /// from when it needs one it did not `#[inject]`.
    pub fn container(&self) -> &Container {
        self.container
    }

    /// The `#[mcp]` host type this operation belongs to.
    pub fn host(&self) -> &'static str {
        self.host
    }

    /// Whether this is a tool call or a prompt fetch.
    pub fn kind(&self) -> McpOperationKind {
        self.kind
    }

    /// The operation's wire name — the tool or prompt the client asked for.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl fmt::Debug for McpOperationContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOperationContext")
            .field("host", &self.host)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Whether an operation's description holds no prose — `str::trim` leaves
/// nothing of it.
///
/// A `const fn` because the reader is the compiler: `#[tools]` refuses a blank
/// description at expansion when the prose is a literal, and a doc line written
/// as a macro (`#[doc = include_str!("tool.md")]`) or a stated
/// `description = <constant>` is a value only a constant evaluation can read —
/// so the expansion emits one asserting it is not blank, and an empty file fails
/// the build with the sentence a missing doc comment gets. `str::trim` is not
/// `const`, so this is its rule written out: Unicode `White_Space`, the
/// property `char::is_whitespace` tests, decoded from UTF-8 one character at a
/// time.
#[doc(hidden)]
pub const fn description_is_blank(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        // A `&str` is valid UTF-8, so the lead byte says how many continuation
        // bytes follow, and each carries six bits.
        let lead = bytes[at];
        let (mut code, width) = match lead {
            0x00..=0x7F => (lead as u32, 1),
            0xC0..=0xDF => ((lead & 0x1F) as u32, 2),
            0xE0..=0xEF => ((lead & 0x0F) as u32, 3),
            _ => ((lead & 0x07) as u32, 4),
        };
        let mut next = 1;
        while next < width {
            code = (code << 6) | (bytes[at + next] & 0x3F) as u32;
            next += 1;
        }
        match char::from_u32(code) {
            Some(character) if character.is_whitespace() => at += width,
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::description_is_blank;

    /// The rule is `str::trim`'s, so `str::trim` is the oracle — over every
    /// scalar value alone and between two of the others.
    #[test]
    fn a_description_is_blank_exactly_when_trim_leaves_nothing() {
        for character in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            let alone = character.to_string();
            assert_eq!(
                description_is_blank(&alone),
                alone.trim().is_empty(),
                "{:?}",
                character,
            );
        }
        for text in [
            "",
            " ",
            "\t\n\r",
            "\u{3000}\u{a0}",
            " x ",
            "\u{3000}é\u{3000}",
            "😀",
        ] {
            assert_eq!(
                description_is_blank(text),
                text.trim().is_empty(),
                "{text:?}"
            );
        }
    }

    /// The point of the `const`: the compiler evaluates it, so a blank
    /// description is a build failure rather than a test one.
    #[test]
    fn the_check_runs_in_a_constant() {
        const {
            assert!(description_is_blank(concat!(" ", "\u{3000}")));
            assert!(!description_is_blank(concat!(" Counts ", "items.")));
        }
    }
}
