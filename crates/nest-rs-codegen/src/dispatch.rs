//! [`DispatchKeys`] — what one host dispatches to one method, refused when two
//! methods claim it.
//!
//! The macro cannot evaluate `#[cfg]`, so it refuses two unconditional claims
//! itself, and for two claims of one key of which either carries a `#[cfg]` it
//! emits a `const` panicking with the same sentence under both claims'
//! conditions: compiled only where both are, it says the framework's refusal
//! wherever rustc would otherwise meet two definitions. A host's impl half is
//! one block, its `impl Discoverable` refusing a second, so no claim escapes
//! the expansion that sees it. A WS lifecycle hook's claim is its trait method,
//! so rustc's own duplicate-definition error stays beside the sentence.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::Ident;

/// One host's keys, as its decorator collects them.
pub struct DispatchKeys {
    /// The impl half as written, e.g. `"#[messages]"`.
    decorator: &'static str,
    /// The clause after the colon of the refusal.
    why: &'static str,
    declared: Vec<Declared>,
}

struct Declared {
    kind: String,
    identity: String,
    scope: Vec<String>,
    key: String,
    method: String,
    cfgs: Vec<TokenStream>,
    span: Span,
}

impl Declared {
    fn unconditional(&self) -> bool {
        self.cfgs.is_empty()
    }

    /// One key claimed twice, in scopes that meet.
    fn meets(&self, kind: &str, identity: &str, scope: &[String]) -> bool {
        self.kind == kind
            && self.identity == identity
            && (self.scope.is_empty()
                || scope.is_empty()
                || self.scope.iter().any(|member| scope.contains(member)))
    }
}

impl DispatchKeys {
    /// An empty set for the host `decorator` expands; `why` completes the refusal.
    pub fn new(decorator: &'static str, why: &'static str) -> Self {
        Self {
            decorator,
            why,
            declared: Vec::new(),
        }
    }

    /// Record that `method` answers `key` under `cfgs`.
    ///
    /// `kind` is the key's family word (`event`, `route`, `tool`), and `key` the
    /// key as a reader wrote it — `#[subscribe_message("ping")]`, `GET /users`.
    /// Two declarations sharing a `kind` and an `identity` claim one key.
    pub fn declare(
        &mut self,
        kind: &str,
        identity: &str,
        key: &str,
        method: &Ident,
        cfgs: &[TokenStream],
        attr: &syn::Attribute,
    ) -> syn::Result<()> {
        self.declare_in(kind, identity, &[], key, method, cfgs, attr)
    }

    /// [`declare`](Self::declare), for a key served only in `scope` — empty
    /// meaning every member, as an unnarrowed route serves every version its
    /// controller mounts.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is one chain a decorator collected; a struct would only rename them"
    )]
    pub fn declare_in(
        &mut self,
        kind: &str,
        identity: &str,
        scope: &[String],
        key: &str,
        method: &Ident,
        cfgs: &[TokenStream],
        attr: &syn::Attribute,
    ) -> syn::Result<()> {
        let method = method.to_string();
        if cfgs.is_empty()
            && let Some(first) = self
                .declared
                .iter()
                .find(|declared| declared.unconditional() && declared.meets(kind, identity, scope))
        {
            return Err(syn::Error::new_spanned(
                attr,
                self.refusal(key, &first.method, &method),
            ));
        }
        // A single token, so rustc points at the declaration, not the decorator.
        let span = attr
            .path()
            .segments
            .last()
            .map_or_else(Span::call_site, |segment| segment.ident.span());
        self.declared.push(Declared {
            kind: kind.to_owned(),
            identity: identity.to_owned(),
            scope: scope.to_vec(),
            key: key.to_owned(),
            method,
            cfgs: cfgs.to_vec(),
            span,
        });
        Ok(())
    }

    fn refusal(&self, key: &str, first: &str, second: &str) -> String {
        format!(
            "`{decorator}` declares `{key}` on `{first}` and again on `{second}`: {why}",
            decorator = self.decorator,
            why = self.why,
        )
    }

    /// For every two claims of one key of which either carries a `#[cfg]`, a
    /// `const` refusing them under both claims' conditions, at the later one.
    pub fn crossings(&self) -> TokenStream {
        let mut crossings = Vec::new();
        for (index, later) in self.declared.iter().enumerate() {
            for earlier in &self.declared[..index] {
                if (earlier.unconditional() && later.unconditional())
                    || !earlier.meets(&later.kind, &later.identity, &later.scope)
                {
                    continue;
                }
                let refusal = self
                    .refusal(&later.key, &earlier.method, &later.method)
                    .replace('{', "{{")
                    .replace('}', "}}");
                let (earlier_cfgs, later_cfgs) = (&earlier.cfgs, &later.cfgs);
                crossings.push(quote_spanned! {later.span=>
                    #(#earlier_cfgs)*
                    #(#later_cfgs)*
                    const _: () = ::core::panic!(#refusal);
                });
            }
        }
        quote!(#(#crossings)*)
    }
}

#[cfg(test)]
mod tests {
    use quote::{format_ident, quote};

    use super::*;

    #[test]
    fn two_unconditional_claims_are_refused_by_the_macro() {
        let mut keys = DispatchKeys::new("#[messages]", "the second would never run");
        let (first, second) = (format_ident!("first"), format_ident!("second"));
        let at: syn::Attribute = syn::parse_quote!(#[subscribe_message("ping")]);
        assert!(
            keys.declare("event", "ping", "ping", &first, &[], &at)
                .is_ok()
        );
        let refusal = keys
            .declare("event", "ping", "ping", &second, &[], &at)
            .err()
            .map(|err| err.to_string());
        assert_eq!(
            refusal.as_deref(),
            Some(
                "`#[messages]` declares `ping` on `first` and again on `second`: the second \
                 would never run"
            ),
        );
    }

    #[test]
    fn claims_in_scopes_that_meet_are_one_key() {
        let at: syn::Attribute = syn::parse_quote!(#[get("/v")]);
        let scope = |members: &[&str]| members.iter().map(|m| (*m).to_owned()).collect::<Vec<_>>();
        let claim = |keys: &mut DispatchKeys, method: &str, members: &[&str]| {
            keys.declare_in(
                "route",
                "get /v",
                &scope(members),
                "GET /v",
                &format_ident!("{method}"),
                &[],
                &at,
            )
        };
        let mut disjoint = DispatchKeys::new("#[routes]", "one handler");
        assert!(claim(&mut disjoint, "one", &["1"]).is_ok());
        assert!(claim(&mut disjoint, "two", &["2"]).is_ok());

        let mut shared = DispatchKeys::new("#[routes]", "one handler");
        assert!(claim(&mut shared, "both", &["1", "2"]).is_ok());
        assert!(claim(&mut shared, "two", &["2"]).is_err());

        let mut every = DispatchKeys::new("#[routes]", "one handler");
        assert!(claim(&mut every, "every", &[]).is_ok());
        assert!(claim(&mut every, "two", &["2"]).is_err());
    }

    #[test]
    fn a_conditional_claim_is_refused_under_both_claims_conditions() {
        let mut keys = DispatchKeys::new("#[messages]", "the second would never run");
        let at: syn::Attribute = syn::parse_quote!(#[subscribe_message("ping")]);
        let cfg = [quote!(#[cfg(feature = "late")])];
        let (a, b) = (format_ident!("a"), format_ident!("b"));
        assert!(keys.declare("event", "ping", "ping", &a, &[], &at).is_ok());
        assert!(keys.declare("event", "ping", "ping", &b, &cfg, &at).is_ok());
        let crossings = keys.crossings().to_string();
        assert_eq!(crossings.matches("const _").count(), 1, "{crossings}");
        assert!(crossings.contains("feature = \"late\""), "{crossings}");
        assert!(
            crossings.contains("declares `ping` on `a` and again on `b`"),
            "{crossings}"
        );
    }

    #[test]
    fn claims_of_two_keys_cross_nothing() {
        let mut keys = DispatchKeys::new("#[messages]", "the second would never run");
        let at: syn::Attribute = syn::parse_quote!(#[subscribe_message("ping")]);
        let cfg = [quote!(#[cfg(all())])];
        let (a, b) = (format_ident!("a"), format_ident!("b"));
        assert!(keys.declare("event", "ping", "ping", &a, &cfg, &at).is_ok());
        assert!(keys.declare("event", "pong", "pong", &b, &cfg, &at).is_ok());
        assert!(keys.crossings().is_empty());
    }
}
