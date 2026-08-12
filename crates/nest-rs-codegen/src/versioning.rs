//! `version = "…"` on an edge that has no address for it — and the one place
//! each transport's own answer is worded.
//!
//! `#[controller(version = "1")]` declares a version because an HTTP route *is*
//! an address a caller selects, and `#[gateway]` follows for the same reason. On
//! the edges below the mount is not selectable that way, so the unity the
//! framework owes the developer is not "accept the key everywhere" — it is that
//! the key either means what it means on HTTP, or the compiler says what this
//! transport does instead. A silently ignored `version` and a bare "unknown
//! key" are the same failure: the developer is left guessing.
//!
//! This is [`crate::attrs::reject_http_only_layers`]'s shape applied to an
//! argument rather than an attribute, and it is worded here rather than in five
//! macro crates for the reason [`crate::pair::DecoratorPair`] and
//! [`crate::posture::PostureRules`] are:
//! `rg 'Edge::' crates/*-macros/src/` names every rejection site, and a sentence
//! that lives once cannot drift into naming a remedy that no longer exists.
//!
//! ```ignore
//! Edge::Schedule.reject_version(&args)?;        // raw decorator argument tokens
//! return Err(Edge::Mcp.refuse_version(&value)); // a value already parsed out
//! ```

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::{Expr, LitStr};

/// The longest version token the framework accepts, declared or stated.
///
/// A version is spliced into a URL path and echoed into an `operationId`, so it
/// is bounded on both sides of the wire. It lived only on the runtime side once,
/// which meant a 40-character `#[controller(version = …)]` compiled, mounted,
/// logged and documented — and was then refused with `400` the moment a caller
/// named it. A declaration the framework accepts and its own wire half rejects
/// is the worst place for the two to disagree.
pub const MAX_VERSION_LEN: usize = 32;

/// Whether `raw` is a version token: bare alphanumerics, `.` and `-`, non-empty,
/// within [`MAX_VERSION_LEN`].
///
/// This is the grammar, worded here so the compile-time half and the wire half
/// are one rule. `nest-rs-http` cannot call it — this crate pulls `syn`, and
/// dragging that into every app's dependency graph is what the umbrella rule
/// forbids — so it carries its own copy of *this function only*, pinned against
/// this one by `versioning::the_wire_grammar_matches_the_declared_grammar`.
pub fn is_valid_version(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= MAX_VERSION_LEN
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Parse a declared version, in either accepted spelling: `version = "1"` or
/// `version = ["1", "2"]`.
///
/// `#[controller]`, `#[version]` and `#[gateway]` all come through here, so the two cannot
/// word "what a version may look like" differently — and it is worth stating
/// once, because a declared version is spliced straight into a URL path.
///
/// The grammar itself is [`is_valid_version`], the
/// same rule the wire-side validator in `nest-rs-http` applies to a version a
/// *caller* states. This half catches your own typo at compile time instead of
/// mounting `/va%2Fb/posts`; what it adds beyond the grammar is the list shape
/// and the duplicate check.
pub fn parse_version_list(value: &Expr, decorator: &str) -> syn::Result<Vec<LitStr>> {
    let literals = match value {
        Expr::Array(array) => array
            .elems
            .iter()
            .map(crate::attrs::expr_str)
            .collect::<syn::Result<Vec<_>>>()?,
        other => vec![crate::attrs::expr_str(other)?],
    };
    if literals.is_empty() {
        return Err(syn::Error::new_spanned(
            value,
            format!("{decorator} `version = []` declares nothing — drop the argument instead"),
        ));
    }
    for (index, literal) in literals.iter().enumerate() {
        let version = literal.value();
        if !is_valid_version(&version) {
            return Err(syn::Error::new_spanned(
                literal,
                format!(
                    "{decorator} version {version:?} is not a path segment — a version is \
                     alphanumerics, `.` and `-` (`1`, `2`, `2024-08-11`), at most {max} \
                     characters, because it is mounted as `/v{version}`",
                    max = MAX_VERSION_LEN,
                ),
            ));
        }
        if literals[..index].iter().any(|seen| seen.value() == version) {
            return Err(syn::Error::new_spanned(
                literal,
                format!("{decorator} declares version {version:?} twice"),
            ));
        }
    }
    Ok(literals)
}

/// An edge whose mount is not an address a client selects, and which therefore
/// answers `version = "…"` with its own alternative instead of accepting it.
///
/// The list is closed on purpose: an edge belongs here only after the question
/// "what does a caller select?" has been answered, and the answer is what
/// [`Edge::answer`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// GraphQL — refused by `#[resolver]`.
    Graphql,
    /// MCP — refused by `#[mcp]`, which spends the word on `serverInfo.version`.
    Mcp,
    /// Queue — refused by `#[processor]`.
    Queue,
    /// Scheduled tasks — refused by `#[scheduled]`.
    Schedule,
    /// In-process events — refused by `#[listeners]`.
    Events,
}

/// One edge's answer, split so neither half can be left blank.
///
/// `because` is the reasoning and `instead` is the remedy, and the second is the
/// one that makes the diagnostic worth reading: an edge that refuses a
/// declaration owes the developer the declaration it does accept.
pub struct VersionAnswer {
    /// The decorator that refuses the key, as written — `"#[resolver]"`.
    pub decorator: &'static str,
    /// Why the HTTP reading of `version` does not apply on this transport.
    pub because: &'static str,
    /// What to write instead. Never empty.
    pub instead: &'static str,
}

impl Edge {
    /// Every edge that refuses `version`, so a variant added later is forced
    /// through the same checks as its siblings.
    pub const ALL: [Self; 5] = [
        Self::Graphql,
        Self::Mcp,
        Self::Queue,
        Self::Schedule,
        Self::Events,
    ];

    /// This edge's decorator, reasoning and remedy.
    pub fn answer(self) -> VersionAnswer {
        match self {
            Self::Graphql => VersionAnswer {
                decorator: "#[resolver]",
                because: "a GraphQL schema is not versioned — one schema, one \
                          introspection, one generated client",
                instead: "Evolve the field and mark the old one deprecated: \
                          `#[graphql(deprecation = \"use `author` instead\")]` beside \
                          the `#[query]` / `#[mutation]` it retires",
            },
            // `#[mcp]` does take a `version`, and it is not this one: it overrides
            // `serverInfo.version` for an endpoint that stands apart. So the
            // answer names both readings — the addressing one the developer
            // probably meant, and the identity one they would otherwise get.
            Self::Mcp => VersionAnswer {
                decorator: "#[mcp]",
                because: "an MCP endpoint is addressed by its whole path, and `version` \
                          here is `serverInfo.version` — the server's own version, not a \
                          segment a client selects",
                instead: "Write the version into the path: `#[mcp(path = \"/mcp/v1\")]`. \
                          The server's own version is not a host's to state — a feature \
                          library knows neither the binary's version nor, on a shared \
                          endpoint, the whole surface — so it is declared once, on the \
                          app's single `McpModule::for_root(McpOptions { server, .. })`",
            },
            Self::Queue => VersionAnswer {
                decorator: "#[processor]",
                because: "a queue is addressed by its name, and versioning that name \
                          splits the consumer group — a deployment decision rather than \
                          a declaration",
                instead: "Name the queue for the version if that is what you mean — \
                          `#[queue(name = \"transcode-v2\", job = TranscodeCommand)] struct \
                          TranscodeV2Queue;` and `#[process(queue = TranscodeV2Queue)]`. \
                          What a job usually needs instead is payload evolution: a \
                          tolerant `Command` shape, not a second address",
            },
            Self::Schedule => VersionAnswer {
                decorator: "#[scheduled]",
                because: "a scheduled task has no caller to select anything — the clock \
                          is the only trigger",
                instead: "Version the work the task calls into, not the tick: the trigger \
                          stays `#[every]` / `#[cron]` / `#[after]`",
            },
            Self::Events => VersionAnswer {
                decorator: "#[listeners]",
                because: "an event listener is in-process — there is no wire, so there is \
                          no address to version",
                instead: "Evolve the event's payload, or publish a new event type beside \
                          the old one and handle both with a second `#[on_event]` method",
            },
        }
    }

    /// The refusal as the developer reads it.
    pub fn version_refusal(self) -> String {
        let VersionAnswer {
            decorator,
            because,
            instead,
        } = self.answer();
        format!("{decorator} declares no client-selectable version: {because}. {instead}")
    }

    /// The refusal spanned on tokens the caller already parsed out — the value
    /// half of a `key = value`, say.
    pub fn refuse_version<T: ToTokens>(self, tokens: T) -> syn::Error {
        syn::Error::new_spanned(tokens, self.version_refusal())
    }

    /// The refusal spanned by hand, for a caller holding a [`Span`] rather than
    /// the tokens it came from.
    pub fn refuse_version_at(self, span: Span) -> syn::Error {
        syn::Error::new(span, self.version_refusal())
    }

    /// Refuse a top-level `version` key in a decorator's raw argument tokens.
    ///
    /// Called **before** the decorator's own unknown-argument arm, which is the
    /// whole point: a generic "takes no arguments" or "unknown key" reads as a
    /// typo and sends the developer looking for the right spelling of something
    /// this transport does not have.
    ///
    /// Only the top level is scanned — a `version` nested inside some other
    /// argument's parentheses belongs to that argument's grammar, not to this
    /// question.
    pub fn reject_version(self, args: &TokenStream) -> syn::Result<()> {
        for tree in args.clone() {
            if let TokenTree::Ident(ident) = tree
                && ident == "version"
            {
                return Err(self.refuse_version_at(ident.span()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    // A variant added later cannot ship a blank sentence: every edge owes a
    // reason *and* a remedy, and both have to reach the message the developer
    // reads. The remedy is the load-bearing half — refusing a declaration
    // without naming the one this transport accepts is the "unknown key" this
    // module exists to replace.
    #[test]
    fn every_edge_names_a_reason_and_an_alternative() {
        for edge in Edge::ALL {
            let VersionAnswer {
                decorator,
                because,
                instead,
            } = edge.answer();
            assert!(!decorator.is_empty(), "{edge:?} names no decorator");
            assert!(!because.is_empty(), "{edge:?} gives no reason");
            assert!(!instead.is_empty(), "{edge:?} names no alternative");

            let message = edge.version_refusal();
            assert!(message.contains(decorator), "{edge:?}: {message}");
            assert!(message.contains(because), "{edge:?}: {message}");
            assert!(
                message.contains(instead),
                "{edge:?} drops its alternative from the message it prints: {message}"
            );
        }
    }

    #[test]
    fn a_version_argument_is_refused_by_name() {
        let err = Edge::Schedule
            .reject_version(&quote!(version = "1"))
            .expect_err("`version` must be refused");
        let message = err.to_string();
        assert!(message.contains("#[scheduled]"), "{message}");
        assert!(
            message.contains("the clock is the only trigger"),
            "{message}"
        );
    }

    #[test]
    fn arguments_this_edge_does_not_own_fall_through() {
        // The scan answers one question. Anything else is the decorator's own
        // grammar to accept or refuse — this must not become a second parser.
        assert!(Edge::Queue.reject_version(&quote!()).is_ok());
        assert!(Edge::Queue.reject_version(&quote!(retries = 3)).is_ok());
    }

    #[test]
    fn a_nested_version_belongs_to_the_argument_that_holds_it() {
        assert!(
            Edge::Mcp
                .reject_version(&quote!(server(version = "1")))
                .is_ok(),
            "only a top-level `version` is this module's question"
        );
    }
}
