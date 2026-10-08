//! `#[mcp]`'s grammar: what a host may declare about the endpoint it joins, and
//! the answer for every field of the server's identity it may not.
//!
//! Here, not in `nest-rs-mcp-macros`, so `nest-rs-mcp`'s suite can hold
//! [`mcp_answers`] against every field of `McpIdentity`.

use proc_macro2::{Ident, Span, TokenStream};
use quote::ToTokens;

use crate::grammar::Grammar;
use crate::versioning::Edge;

/// The keys a host declares; a key naming a field of the server's identity is
/// refused by name in [`SERVER_FIELDS`].
const KEYS: [&str; 3] = ["path", "name", "title"];

/// `#[mcp]`'s grammar: a server-identity key is refused naming where the app
/// declares it; any other unknown key gets the list of what remains.
pub const MCP_GRAMMAR: Grammar = Grammar::new("mcp", &KEYS).elsewhere(server_field);

/// Whether `#[mcp]` answers `key` — takes it, or refuses it naming its owner —
/// rather than leaving it to the bare unknown-key sentence; `version` is
/// answered by [`Edge::Mcp`].
pub fn mcp_answers(key: &str) -> bool {
    let written: TokenStream = Ident::new(key, Span::call_site()).into_token_stream();
    KEYS.contains(&key)
        || server_field(key).is_some()
        || Edge::Mcp.reject_version(&written).is_err()
}

/// Where a host's own prose goes, for the fields with a per-operation twin.
const TOOL_DESCRIPTION: &str =
    "What this host's tools *do* belongs to each #[tool(description = \"…\")]";

/// A field of the server's identity: real, settable, and the **app's** to set.
struct ServerField {
    /// The key as a host writes it, the identity field's own name.
    key: &'static str,
    /// The `McpIdentity` call that takes it, spelled into the remedy.
    declares: &'static str,
    /// What they may have meant instead; empty when nothing.
    instead: &'static str,
}

/// Every identity field the app owns, refused by name; `version` is refused
/// first by [`Edge::Mcp`].
const SERVER_FIELDS: [ServerField; 4] = [
    ServerField {
        key: "description",
        declares: "description(\"…\")",
        instead: TOOL_DESCRIPTION,
    },
    ServerField {
        key: "website_url",
        declares: "website_url(\"…\")",
        instead: "",
    },
    ServerField {
        key: "icons",
        declares: "icons([…])",
        instead: "",
    },
    ServerField {
        key: "instructions",
        declares: "instructions(\"…\")",
        instead: TOOL_DESCRIPTION,
    },
];

/// The identity field a `#[mcp]` argument names, if it names one, keyed off
/// the path alone.
fn server_field(key: &str) -> Option<String> {
    SERVER_FIELDS
        .iter()
        .find(|field| field.key == key)
        .map(ServerField::refusal)
}

impl ServerField {
    /// The refusal: whose the field is, and the one call that takes it.
    fn refusal(&self) -> String {
        let Self {
            key,
            declares,
            instead,
        } = self;
        let mut message = format!(
            "#[mcp] takes no `{key}` — that describes the server, not one host, so it is \
             declared once: McpModule::for_root(McpOptions {{ server: \
             Some(McpIdentity::new(name, version).{declares}), ..Default::default() }})",
        );
        if !instead.is_empty() {
            message.push_str(". ");
            message.push_str(instead);
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use super::{KEYS, SERVER_FIELDS, mcp_answers};

    #[test]
    fn every_refused_field_names_its_key_and_the_seam_that_takes_it() {
        for field in &SERVER_FIELDS {
            let message = field.refusal();
            assert!(message.contains(field.key), "{message}");
            assert!(
                message.contains(field.declares),
                "{} names no call to paste: {message}",
                field.key,
            );
            assert!(
                message.contains("McpModule::for_root"),
                "{} names no seam: {message}",
                field.key,
            );
            assert!(
                !KEYS.contains(&field.key),
                "`{}` is refused by name, so the accepted-key list must not offer it",
                field.key,
            );
        }
    }

    #[test]
    fn a_key_is_answered_by_the_grammar_by_name_or_by_the_edge() {
        for key in ["path", "title", "icons", "version"] {
            assert!(mcp_answers(key), "`{key}` has an answer");
        }
        assert!(!mcp_answers("colour"), "a key nobody owns has none");
    }
}
