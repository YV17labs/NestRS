//! `version = "…"` on an edge that has no address for it — and the one place
//! each transport's own answer is worded.
//!
//! An HTTP route or a gateway is an address a caller selects; on the edges
//! below it is not, so `version` there is refused naming what the transport
//! does instead.
//!
//! ```
//! # use nest_rs_codegen::Edge;
//! # fn refuse(args: proc_macro2::TokenStream, value: syn::LitStr) -> syn::Result<()> {
//! Edge::Schedule.reject_version(&args)?;        // raw decorator argument tokens
//! return Err(Edge::Mcp.refuse_version(&value)); // a value already parsed out
//! # }
//! # let declared = refuse(quote::quote!(version = "1"), syn::parse_quote!("1"));
//! # assert!(declared.is_err_and(|e| e.to_string().contains("#[scheduled]")));
//! # let parsed = refuse(quote::quote!(), syn::parse_quote!("1"));
//! # assert!(parsed.is_err_and(|e| e.to_string().contains("#[mcp]")));
//! ```

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Lit, LitStr, Token};

/// The longest version token the framework accepts, declared or stated — the
/// same bound on both sides of the wire.
pub const MAX_VERSION_LEN: usize = 32;

/// Whether `raw` is a version token: bare alphanumerics, `.` and `-`, non-empty,
/// within [`MAX_VERSION_LEN`].
///
/// `nest-rs-http` cannot depend on this crate (it pulls `syn`), so it carries a
/// copy pinned by `versioning::the_wire_grammar_matches_the_declared_grammar`.
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
/// The grammar is [`is_valid_version`]; this adds the list shape and the
/// duplicate check.
pub fn parse_version_list(value: &Expr, decorator: &str) -> syn::Result<Vec<LitStr>> {
    // `decorator` arrives as `"#[controller]"`; the shared sentence brackets the
    // name itself, so it is handed the bare word.
    let attr = decorator.trim_start_matches("#[").trim_end_matches(']');
    // A forwarded `$v:expr` list arrives as `Expr::Group(Array)`.
    let literals = match crate::ungrouped_expr(value) {
        Expr::Array(array) => array
            .elems
            .iter()
            .map(|elem| crate::args::require_str_lit(elem, attr, "version", "1"))
            .collect::<syn::Result<Vec<_>>>()?,
        Expr::Lit(ExprLit {
            lit: Lit::Str(single),
            ..
        }) => vec![single.clone()],
        other => {
            return Err(syn::Error::new_spanned(
                other,
                crate::args::takes_value(
                    attr,
                    Some("version"),
                    "a string literal or a list of them, e.g. `version = \"1\"` or \
                     `version = [\"1\", \"2\"]`",
                ),
            ));
        }
    };
    if literals.is_empty() {
        return Err(syn::Error::new_spanned(
            value,
            format!(
                "{} declares nothing — drop the argument instead",
                crate::args::site(attr, Some("version = []"))
            ),
        ));
    }
    check_versions(&literals, attr, Some("version"))?;
    Ok(literals)
}

/// The route's own spelling, `#[version("1", "2")]` — the same grammar as
/// [`parse_version_list`], read off the attribute's positional arguments.
///
/// Its refusals name the attribute alone (``#[version]: "a b" is not a path
/// segment``), which has no key.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
pub fn parse_version_args(attr: &syn::Attribute) -> syn::Result<Vec<LitStr>> {
    const ROUTE: &str = "version";
    let refused = |at: &dyn ToTokens| {
        syn::Error::new_spanned(
            at,
            crate::args::takes_value(
                ROUTE,
                None,
                "the route's versions as string literals, e.g. `#[version(\"1\", \"2\")]`",
            ),
        )
    };
    let listed = attr
        .parse_args_with(Punctuated::<Expr, Token![,]>::parse_terminated)
        .map_err(|_| refused(attr))?;
    let literals = listed
        .iter()
        .map(|elem| match crate::ungrouped::ungrouped_expr(elem) {
            Expr::Lit(ExprLit {
                lit: Lit::Str(literal),
                ..
            }) => Ok(literal.clone()),
            other => Err(refused(other)),
        })
        .collect::<syn::Result<Vec<_>>>()?;
    if literals.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            format!(
                "{} declares nothing — drop the attribute instead",
                crate::args::site(ROUTE, None)
            ),
        ));
    }
    check_versions(&literals, ROUTE, None)?;
    Ok(literals)
}

/// The grammar and the duplicate check, over versions already read as literals
/// and refused at `#[attr]`'s `key` — `None` for the positional spelling.
fn check_versions(literals: &[LitStr], attr: &str, key: Option<&str>) -> syn::Result<()> {
    for (index, literal) in literals.iter().enumerate() {
        let version = literal.value();
        if !is_valid_version(&version) {
            return Err(syn::Error::new_spanned(
                literal,
                format!(
                    "{}: {version:?} is not a path segment — a version is alphanumerics, `.` \
                     and `-` (`1`, `2`, `2024-08-11`), at most {max} characters, because it is \
                     mounted as `/v{version}`",
                    crate::args::site(attr, key),
                    max = MAX_VERSION_LEN,
                ),
            ));
        }
        if literals[..index].iter().any(|seen| seen.value() == version) {
            return Err(syn::Error::new_spanned(
                literal,
                format!(
                    "{}: {version:?} is listed twice",
                    crate::args::site(attr, key)
                ),
            ));
        }
    }
    Ok(())
}

/// An edge whose mount is not an address a client selects, and which therefore
/// answers `version = "…"` with its own alternative instead of accepting it.
///
/// An edge belongs here once [`Edge::answer`] can say what a caller selects
/// instead.
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
            // Names both readings: addressing, and `serverInfo.version`.
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
    /// Call it **before** the decorator's own unknown-argument arm. Only the top
    /// level is scanned.
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

    #[test]
    fn a_forwarded_version_list_is_read_through_its_group() {
        let list: Expr = syn::parse_quote!(["1", "2"]);
        let forwarded = Expr::Group(syn::ExprGroup {
            attrs: Vec::new(),
            group_token: Default::default(),
            expr: Box::new(list),
        });
        let versions =
            parse_version_list(&forwarded, "#[controller]").expect("a forwarded list is a list");
        let read: Vec<String> = versions.iter().map(LitStr::value).collect();
        assert_eq!(read, ["1", "2"]);

        let wrong: Expr = syn::parse_quote!(1);
        let Err(refusal) = parse_version_list(&wrong, "#[controller]") else {
            panic!("a number is no version");
        };
        let refusal = refusal.to_string();
        assert!(
            refusal.contains("or a list of them"),
            "the refusal offers the list spelling: {refusal}",
        );
    }

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
