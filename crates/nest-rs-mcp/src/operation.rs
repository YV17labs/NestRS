//! What one MCP operation *is*, for the layers that run around it.
//!
//! [`McpOperationContext`] carries no arguments: deciding access from a payload
//! is a pipe's job, never a guard's.

use std::fmt;

use nest_rs_core::Container;

/// The container serving the current MCP operation, read off the ambient request
/// scope; `None` outside a dispatch nested under the HTTP transport edge.
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

/// One MCP operation, as a [`Guard`](https://docs.rs/nest-rs-guards) sees it.
///
/// The caller's `Ability` is ambient by the time a guard runs, read with
/// `nest_rs_authz::current_ability` as on any other transport.
pub struct McpOperationContext<'a> {
    container: &'a Container,
    host: &'static str,
    kind: McpOperationKind,
    name: &'static str,
}

impl<'a> McpOperationContext<'a> {
    /// Describe the operation about to run; emitted by `#[tools]`.
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
/// nothing of it. `const` so `#[tools]` can assert a non-literal description
/// (`include_str!`, a constant) at build time, which `str::trim` cannot.
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

    /// `str::trim` is the oracle.
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

    #[test]
    fn the_check_runs_in_a_constant() {
        const {
            assert!(description_is_blank(concat!(" ", "\u{3000}")));
            assert!(!description_is_blank(concat!(" Counts ", "items.")));
        }
    }
}
