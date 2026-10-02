//! `#[mcp]`'s grammar: what a host may declare about the endpoint it joins, and
//! the answer for every field of the server's identity it may not.
//!
//! Here rather than in `nest-rs-mcp-macros` because the answer has to be held
//! against `McpIdentity`, which only `nest-rs-mcp` can see whole: its unit
//! suite destructures the identity without `..` and asks [`mcp_answers`] about
//! every field, so a field added there does not compile until it is listed, and
//! does not pass until this table answers it. A proc-macro crate exports
//! nothing but macros, so the table could not be reached where it was.

use proc_macro2::{Ident, Span, TokenStream};
use quote::ToTokens;

use crate::grammar::Grammar;
use crate::versioning::Edge;

/// The keys a host declares. Every other key that names a field of the
/// server's identity is refused by name in [`SERVER_FIELDS`], so a key reaching
/// the shared unknown-argument sentence is one nothing in the framework has a
/// home for — the only case a list of spellings actually helps.
///
/// `path` is the endpoint the host joins; `name` and `title` are the pair a
/// host can honestly say about it — *which endpoint stands apart*.
const KEYS: [&str; 3] = ["path", "name", "title"];

/// `#[mcp]`'s grammar. Two different answers to a key outside it, and telling
/// them apart is the point: a key naming a field of the server's identity is a
/// key that *exists* — it is declared by the app, and the sentence says where —
/// while anything else is nobody's, and gets the list of what remains.
pub const MCP_GRAMMAR: Grammar = Grammar::new("mcp", &KEYS).elsewhere(server_field);

/// Whether `#[mcp]` answers `key` — takes it, or refuses it naming its owner —
/// rather than leaving it to the bare unknown-key sentence. `version` is
/// answered one step before the grammar, by [`Edge::Mcp`], in the words every
/// edge without a client-selectable version shares.
pub fn mcp_answers(key: &str) -> bool {
    let written: TokenStream = Ident::new(key, Span::call_site()).into_token_stream();
    KEYS.contains(&key)
        || server_field(key).is_some()
        || Edge::Mcp.reject_version(&written).is_err()
}

/// Where a host's own prose goes, for the fields whose per-operation twin is
/// what a developer reaching for them usually meant. A server-level field
/// refused without it answers "not here" and not "there".
const TOOL_DESCRIPTION: &str =
    "What this host's tools *do* belongs to each #[tool(description = \"…\")]";

/// A field of the server's identity: real, settable, and the **app's** to set.
///
/// Each is a key a host may plausibly write, and writing it is not a typo — the
/// field exists, it is just declared at the seam that can honestly state it,
/// since on an endpoint several features share no single host sees the whole.
/// So each owes the *same shape of answer* `version` already gives: a sentence
/// naming the seam that takes it.
struct ServerField {
    /// The key as a host writes it, which is the identity field's own name.
    key: &'static str,
    /// The `McpIdentity` call that takes it, spelled into the remedy so the
    /// sentence carries a line the developer can paste.
    declares: &'static str,
    /// What they may have meant instead, when the field has a per-operation
    /// twin. Empty when it has none.
    instead: &'static str,
}

/// Every identity field the app owns, refused by name. `version` is absent
/// because [`Edge::Mcp`] refuses it first, and says more than "the app declares
/// it".
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

/// The identity field a `#[mcp]` argument names, if it names one. Keyed off the
/// path alone, so a bare `icons` and an `icons = [..]` get the same answer — a
/// host that reached for the key learns where it lives either way.
fn server_field(key: &str) -> Option<String> {
    SERVER_FIELDS
        .iter()
        .find(|field| field.key == key)
        .map(ServerField::refusal)
}

impl ServerField {
    /// The refusal as the developer reads it: whose the field is, and the one
    /// call that takes it. Worded once for every row — three spellings of one
    /// sentence is how the list fell behind the struct in the first place.
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

    /// A row added later cannot ship a sentence that names nothing: the key it
    /// refuses, the call that takes it, and the seam that call belongs to all
    /// have to reach the message the developer reads.
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

    /// The three answers, and the silence `mcp_answers` exists to tell apart
    /// from them.
    #[test]
    fn a_key_is_answered_by_the_grammar_by_name_or_by_the_edge() {
        for key in ["path", "title", "icons", "version"] {
            assert!(mcp_answers(key), "`{key}` has an answer");
        }
        assert!(!mcp_answers("colour"), "a key nobody owns has none");
    }
}
