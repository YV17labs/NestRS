//! [`Grammar`] — the one reader of a decorator's `key = value` arguments.
//!
//! Every decorator whose arguments are a list of keys owes the same three
//! refusals: a key it does not take ([`unknown_argument`]), a key written twice
//! ([`duplicate_argument`]), and a key written bare that needs a value
//! ([`needs_a_value`]). They used to be issued at each decorator's own loop —
//! seventeen loops over three argument shapes — so a refusal was something a
//! loop could leave out, and the repeat that kept the last of two
//! `via = "…"` keys was exactly that. Here they are issued by the loop itself,
//! once, before the decorator sees a key: the closure a decorator hands to
//! [`Grammar::parse`] receives only keys of its own grammar, each the first time
//! it is written.
//!
//! What the closure reads after the key is its own business — a value through
//! [`Arg::value`], [`Arg::expr`] or [`Arg::str_lit`], a parenthesised list off
//! [`Arg::input`], or nothing for a flag — because the values differ (a type, a
//! list, an expression) and the refusals do not.
//!
//! `syn::meta::parser` and `syn::Attribute::parse_nested_meta` are the
//! alternative this replaces, and the root `clippy.toml` refuses both outside
//! the one site that reads an attribute whose grammar is another library's.

use proc_macro2::{TokenStream, TokenTree};
use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::{Expr, Ident, LitStr, Token};

use crate::args::{duplicate_argument, needs_a_value, require_str_lit, unknown_argument};

/// One decorator's argument grammar: its name and the keys it takes, in the
/// order its refusals list them.
///
/// Declared as a `const` beside the decorator's parser, so the decorator's name
/// and its key list are written once — they were written twice at every loop,
/// once to refuse and once to read.
#[derive(Clone, Copy, Debug)]
pub struct Grammar {
    attr: &'static str,
    keys: &'static [&'static str],
    /// The key whose own list these keys are written in — `throttle` for
    /// `throttle(limit = …, window = …)` — named in every sentence as
    /// `parent(key)`, the way the developer wrote it.
    parent: Option<&'static str>,
    /// What a repeat of a key is told beside the shared sentence, where the
    /// grammar has a remedy for it.
    remedies: &'static [(&'static str, &'static str)],
    /// The refusal of a key this grammar does not take but the framework knows
    /// elsewhere — another job member's, the MCP server's own identity — which
    /// is meaningless here rather than misspelled, so it is told whose it is.
    elsewhere: fn(&str) -> Option<String>,
    /// The sentence for a key written bare, given the decorator and the key as
    /// spelled.
    bare: fn(&str, &str) -> String,
}

/// No key is known elsewhere.
fn nowhere(_: &str) -> Option<String> {
    None
}

impl Grammar {
    /// `#[attr(…)]`, taking `keys`.
    pub const fn new(attr: &'static str, keys: &'static [&'static str]) -> Self {
        Self {
            attr,
            keys,
            parent: None,
            remedies: &[],
            elsewhere: nowhere,
            bare: needs_a_value,
        }
    }

    /// The same grammar read inside `parent(…)`, so its sentences name a key
    /// as `parent(key)`.
    pub const fn under(self, parent: &'static str) -> Self {
        Self {
            parent: Some(parent),
            ..self
        }
    }

    /// Append each `(key, remedy)` to the repeat sentence of that key —
    /// `version`'s "write one `version = [\"1\", \"2\"]`" — so the refusal stays
    /// the shared one and the grammar's own advice follows it.
    pub const fn with_remedies(self, remedies: &'static [(&'static str, &'static str)]) -> Self {
        Self { remedies, ..self }
    }

    /// Refuse a key outside this grammar with `elsewhere`'s sentence when it
    /// has one, rather than as unknown.
    pub const fn elsewhere(self, elsewhere: fn(&str) -> Option<String>) -> Self {
        Self { elsewhere, ..self }
    }

    /// Word a bare key with `bare(attr, key)` instead of the shared
    /// needs-a-value sentence.
    pub const fn bare(self, bare: fn(&str, &str) -> String) -> Self {
        Self { bare, ..self }
    }

    /// The keys this grammar takes, in declaration order.
    pub const fn keys(&self) -> &'static [&'static str] {
        self.keys
    }

    /// Read `key [= value | (list)], …` off `input` to its end, handing each key
    /// to `each` after refusing it if it is unknown or written before.
    ///
    /// After `each` returns, the next token must be a `,` or the end, so a value
    /// `each` left unread is refused rather than skipped.
    pub fn parse(
        &self,
        input: ParseStream<'_>,
        mut each: impl FnMut(Arg<'_>) -> syn::Result<()>,
    ) -> syn::Result<()> {
        let mut written: Vec<&'static str> = Vec::new();
        while !input.is_empty() {
            let ident = self.key_at(input)?;
            let key = self.take(&mut written, &ident)?;
            each(Arg {
                grammar: *self,
                key,
                ident,
                input,
            })?;
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(())
    }

    /// [`parse`](Self::parse) over a whole argument token stream — a
    /// decorator's `args`.
    pub fn parse2(
        &self,
        tokens: TokenStream,
        each: impl FnMut(Arg<'_>) -> syn::Result<()>,
    ) -> syn::Result<()> {
        (|input: ParseStream<'_>| self.parse(input, each)).parse2(tokens)
    }

    /// [`parse`](Self::parse) over an attribute's parenthesised arguments —
    /// a marker such as `#[api(…)]` that a decorator reads off a method.
    pub fn parse_attr(
        &self,
        attr: &syn::Attribute,
        each: impl FnMut(Arg<'_>) -> syn::Result<()>,
    ) -> syn::Result<()> {
        attr.parse_args_with(|input: ParseStream<'_>| self.parse(input, each))
    }

    /// Refuse each key in `written` as [`parse`](Self::parse) would — unknown,
    /// or written twice — for a grammar that mixes positional arguments with
    /// keys and so reads its own list: `#[authorize(Update, bind = Service)]`.
    pub fn take_all<'k>(&self, written: impl IntoIterator<Item = &'k Ident>) -> syn::Result<()> {
        let mut taken = Vec::new();
        for ident in written {
            self.take(&mut taken, ident)?;
        }
        Ok(())
    }

    /// The key at the head of `input`, any identifier keywords included —
    /// anything else is refused as an argument this grammar does not take,
    /// named as written.
    fn key_at(&self, input: ParseStream<'_>) -> syn::Result<Ident> {
        if input.peek(Ident::peek_any) {
            return input.call(Ident::parse_any);
        }
        let written: TokenTree = input.parse()?;
        Err(syn::Error::new_spanned(
            &written,
            unknown_argument(self.attr, &written.to_string(), self.keys),
        ))
    }

    /// `ident` as one of this grammar's keys, refused when it is not one or
    /// was written before.
    fn take(&self, written: &mut Vec<&'static str>, ident: &Ident) -> syn::Result<&'static str> {
        let name = ident.unraw().to_string();
        let spelled = self.spelled(&name);
        let Some(key) = self.keys.iter().copied().find(|key| *key == name) else {
            let refusal = (self.elsewhere)(&name)
                .unwrap_or_else(|| unknown_argument(self.attr, &spelled, self.keys));
            return Err(syn::Error::new_spanned(ident, refusal));
        };
        if written.contains(&key) {
            let sentence = duplicate_argument(self.attr, &spelled);
            let sentence = match self.remedies.iter().find(|(remedied, _)| *remedied == key) {
                Some((_, remedy)) => format!("{sentence} — {remedy}"),
                None => sentence,
            };
            return Err(syn::Error::new_spanned(ident, sentence));
        }
        written.push(key);
        Ok(key)
    }

    /// `key`, or `parent(key)` inside a parent's list.
    fn spelled(&self, key: &str) -> String {
        match self.parent {
            Some(parent) => format!("{parent}({key})"),
            None => key.to_owned(),
        }
    }

    fn needs_a_value(&self, key: &str) -> String {
        (self.bare)(self.attr, &self.spelled(key))
    }
}

/// One key of a [`Grammar`], handed to the decorator with the stream still at
/// what follows it.
pub struct Arg<'a> {
    grammar: Grammar,
    key: &'static str,
    ident: Ident,
    input: ParseStream<'a>,
}

impl<'a> Arg<'a> {
    /// The key, as the grammar declares it — match on it.
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// The key as written, for a span.
    pub fn ident(&self) -> &Ident {
        &self.ident
    }

    /// The stream at what follows the key, for a key that takes a list or
    /// nothing at all.
    pub fn input(&self) -> ParseStream<'a> {
        self.input
    }

    /// The stream past the key's `=`, refused with the shared needs-a-value
    /// sentence when the key was written bare.
    pub fn value(&self) -> syn::Result<ParseStream<'a>> {
        if !self.input.peek(Token![=]) {
            return Err(syn::Error::new(
                self.ident.span(),
                self.grammar.needs_a_value(self.key),
            ));
        }
        self.input.parse::<Token![=]>()?;
        Ok(self.input)
    }

    /// The key's value as an expression.
    pub fn expr(&self) -> syn::Result<Expr> {
        self.value()?.parse()
    }

    /// The key's value as a string literal, refused naming the decorator, the
    /// key and `example` — see [`require_str_lit`].
    pub fn str_lit(&self, example: &str) -> syn::Result<LitStr> {
        let value = self.expr()?;
        require_str_lit(
            &value,
            self.grammar.attr,
            &self.grammar.spelled(self.key),
            example,
        )
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    const PROBE: Grammar =
        Grammar::new("probe", &["path", "version"]).with_remedies(&[("version", "write one list")]);

    /// What `PROBE` reads off `tokens`: each key with its value as written, or
    /// the refusal.
    fn read(grammar: Grammar, tokens: TokenStream) -> Result<Vec<(&'static str, String)>, String> {
        let mut read = Vec::new();
        grammar
            .parse2(tokens, |arg| {
                let value = arg.expr()?;
                read.push((arg.key(), quote!(#value).to_string()));
                Ok(())
            })
            .map(|()| read)
            .map_err(|refusal| refusal.to_string())
    }

    /// Canaries for the root `clippy.toml`: an entry whose path stops resolving
    /// is only a warning, which `-D warnings` does not promote, so each entry is
    /// held by an expectation that fails the lint run once it is unmet.
    #[test]
    fn the_syn_meta_readers_stay_refused() {
        #[expect(clippy::disallowed_methods, reason = "canary for clippy.toml")]
        let parser = syn::meta::parser(|_| Ok(()));
        assert!(parser.parse2(TokenStream::new()).is_ok());
        let attr: syn::Attribute = syn::parse_quote!(#[probe()]);
        #[expect(clippy::disallowed_methods, reason = "canary for clippy.toml")]
        let read = attr.parse_nested_meta(|_| Ok(()));
        assert!(read.is_ok());
    }

    #[test]
    fn every_key_is_handed_over_in_order_with_its_value() {
        assert_eq!(
            read(PROBE, quote!(version = "1", path = "/users",)),
            Ok(vec![
                ("version", "\"1\"".to_owned()),
                ("path", "\"/users\"".to_owned())
            ]),
        );
        assert_eq!(read(PROBE, quote!()), Ok(vec![]));
    }

    #[test]
    fn an_unknown_key_is_refused_listing_the_grammar() {
        assert_eq!(
            read(PROBE, quote!(path = "/", pth = "/")),
            Err("unknown #[probe] argument `pth`; expected `path` or `version`".to_owned()),
        );
        assert_eq!(
            read(PROBE, quote!("/users")),
            Err("unknown #[probe] argument `\"/users\"`; expected `path` or `version`".to_owned()),
        );
    }

    #[test]
    fn a_repeated_key_is_refused_with_its_remedy() {
        assert_eq!(
            read(PROBE, quote!(path = "/a", path = "/b")),
            Err("#[probe] takes at most one `path`".to_owned()),
        );
        assert_eq!(
            read(PROBE, quote!(version = "1", version = "2")),
            Err("#[probe] takes at most one `version` — write one list".to_owned()),
        );
    }

    #[test]
    fn a_bare_key_needs_a_value() {
        assert_eq!(
            read(PROBE, quote!(path)),
            Err("#[probe] `path` needs a value — write `path = ...`".to_owned()),
        );
    }

    #[test]
    fn keys_a_mixed_grammar_read_itself_are_taken_the_same_way() {
        let keys: Vec<Ident> = vec![syn::parse_quote!(path), syn::parse_quote!(version)];
        assert!(PROBE.take_all(&keys).is_ok());
        let repeated: Vec<Ident> = vec![syn::parse_quote!(path), syn::parse_quote!(path)];
        assert_eq!(
            PROBE
                .take_all(&repeated)
                .map_err(|refusal| refusal.to_string()),
            Err("#[probe] takes at most one `path`".to_owned()),
        );
        let unknown: Vec<Ident> = vec![syn::parse_quote!(via)];
        assert_eq!(
            PROBE
                .take_all(&unknown)
                .map_err(|refusal| refusal.to_string()),
            Err("unknown #[probe] argument `via`; expected `path` or `version`".to_owned()),
        );
    }

    #[test]
    fn a_nested_grammar_names_its_keys_inside_the_parent() {
        let nested = Grammar::new("process", &["limit"]).under("throttle");
        assert_eq!(
            read(nested, quote!(limit)),
            Err(
                "#[process] `throttle(limit)` needs a value — write `throttle(limit = ...)`"
                    .to_owned()
            ),
        );
        assert_eq!(
            read(nested, quote!(limit = 1, limit = 2)),
            Err("#[process] takes at most one `throttle(limit)`".to_owned()),
        );
        assert_eq!(
            read(nested, quote!(limt = 1)),
            Err("unknown #[process] argument `throttle(limt)`; expected `limit`".to_owned()),
        );
    }

    #[test]
    fn an_attributes_arguments_are_read_the_same_way() {
        let attr: syn::Attribute = syn::parse_quote!(#[probe(path = "/a", path = "/b")]);
        let refusal = PROBE
            .parse_attr(&attr, |arg| arg.expr().map(drop))
            .map_err(|refusal| refusal.to_string());
        assert_eq!(refusal, Err("#[probe] takes at most one `path`".to_owned()));
    }

    #[test]
    fn a_value_left_unread_is_refused_rather_than_skipped() {
        let refusal = PROBE
            .parse2(quote!(path = "/", version = "1"), |_| Ok(()))
            .map_err(|refusal| refusal.to_string());
        assert_eq!(refusal, Err("expected `,`".to_owned()));
    }

    #[test]
    fn a_job_member_names_another_members_key_and_its_bare_transactional() {
        let every = crate::JobDecorator::Every.grammar();
        assert_eq!(
            read(every, quote!(retries = 3)),
            Err("#[every] takes no `retries`: a tick's retry is the next occurrence".to_owned()),
        );
        let bare = read(every, quote!(transactional)).expect_err("bare");
        assert!(
            bare.starts_with(
                "#[every] `transactional` needs a value — write `transactional = true`"
            ),
            "{bare}"
        );
    }
}
