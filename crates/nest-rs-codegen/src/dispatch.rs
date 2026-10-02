//! [`DispatchKeys`] — what one host dispatches to one method, refused when two
//! methods claim it.
//!
//! A WS event, a connection hook, an HTTP verb and path, an MCP tool or prompt
//! name: each is a key a host answers with exactly one method. A second method
//! claiming the key never runs — or worse, runs under the first method's layers
//! — so two declarations of one key are a compile error.
//!
//! **Who can see the fact decides where it is refused.** An attribute macro reads
//! a method before its `#[cfg]` is evaluated, so the expansion cannot tell two
//! conditions that both hold from two that exclude each other: `#[cfg(all())]`
//! and no condition at all differ as written and agree as evaluated. The macro
//! refuses what it can see — two declarations with no condition on either — and
//! hands the rest to rustc, which evaluates conditions: every declaration emits a
//! unit associated constant under its method's conditions, named for the key, so
//! two that are both compiled are `E0201`/`E0592` on the attribute that declared
//! them, and two that exclude each other are one constant.
//!
//! **A key may be served in a scope** — an HTTP route under a subset of its
//! controller's versions. Two claims collide when their scopes meet: two scoped
//! claims share a member, or one claim is unscoped and so serves every member.
//! A scoped claim's marker is one per member, so two that share a member are the
//! same `E0592`; an unscoped claim beside a scoped one meets it whatever the
//! scope holds, and the macro cannot see the conditions, so that pair is a
//! `const` evaluated under both claims' conditions, failing with the refusal's
//! own sentence when both are compiled.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::Ident;

/// What rustc finds when two claims of one key are both compiled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Collision {
    /// Nothing of the claim's own collides, so [`DispatchKeys::markers`] emits a
    /// constant that does.
    Marker,
    /// The claim emits an item named for the key — a WS connection hook is the
    /// trait method `on_connect` — which is already a duplicate definition, so a
    /// marker would only say it twice.
    Item,
}

/// One host's keys, as its decorator collects them.
pub struct DispatchKeys {
    /// The impl half as written, e.g. `"#[messages]"`.
    decorator: &'static str,
    /// What the host does with each key and why a second claim is wrong, the
    /// clause after the colon of the refusal.
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
    marker: Option<TokenStream>,
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
    /// `identity` is what makes two keys the same: two declarations sharing a
    /// `kind` and an `identity` claim one key. Refused here when neither carries a
    /// condition; otherwise left to rustc, at `attr` — through a marker, or through
    /// the claim's own item, as `collision` says.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is one chain a decorator collected; a struct would only rename them"
    )]
    pub fn declare(
        &mut self,
        collision: Collision,
        kind: &str,
        identity: &str,
        key: &str,
        method: &Ident,
        cfgs: &[TokenStream],
        attr: &syn::Attribute,
    ) -> syn::Result<()> {
        self.declare_in(collision, kind, identity, &[], key, method, cfgs, attr)
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
        collision: Collision,
        kind: &str,
        identity: &str,
        scope: &[String],
        key: &str,
        method: &Ident,
        cfgs: &[TokenStream],
        attr: &syn::Attribute,
    ) -> syn::Result<()> {
        let method = method.to_string();
        let unconditional = cfgs.is_empty();
        if unconditional
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
        // The attribute's own name carries the span: a single token, so rustc
        // points at the declaration a reader wrote rather than at the decorator.
        let span = attr
            .path()
            .segments
            .last()
            .map_or_else(Span::call_site, |segment| segment.ident.span());
        let names: Vec<Ident> = if scope.is_empty() {
            vec![Ident::new(
                &marker_name(self.decorator, kind, identity),
                span,
            )]
        } else {
            scope
                .iter()
                .map(|member| {
                    let scoped = format!("{identity} @{member}");
                    Ident::new(&marker_name(self.decorator, kind, &scoped), span)
                })
                .collect()
        };
        self.declared.push(Declared {
            kind: kind.to_owned(),
            identity: identity.to_owned(),
            scope: scope.to_vec(),
            key: key.to_owned(),
            method,
            cfgs: cfgs.to_vec(),
            span,
            marker: (collision == Collision::Marker).then(|| {
                names
                    .iter()
                    .map(|name| {
                        quote_spanned! {span=>
                            #(#cfgs)*
                            #[doc(hidden)]
                            #[allow(non_upper_case_globals, dead_code)]
                            const #name: () = ();
                        }
                    })
                    .collect()
            }),
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

    /// Every marker, as associated constants of `self_ty` — scoped to the host,
    /// so two hosts answering one key each are two constants of two types.
    pub fn markers(&self, self_ty: &syn::Type, generics: &syn::Generics) -> TokenStream {
        let markers: Vec<&TokenStream> = self
            .declared
            .iter()
            .filter_map(|declared| declared.marker.as_ref())
            .collect();
        let (impl_generics, _, where_clause) = generics.split_for_impl();
        let markers = (!markers.is_empty()).then(|| {
            quote! {
                impl #impl_generics #self_ty #where_clause {
                    #(#markers)*
                }
            }
        });
        // An unscoped claim beside a scoped one of the same key: no pair of
        // marker names can meet, so the pair is refused by a `const` compiled
        // only where both claims are. Two unconditional claims never reach here —
        // `declare_in` refused the second.
        let mut crossings = Vec::new();
        for (index, later) in self.declared.iter().enumerate() {
            for earlier in &self.declared[..index] {
                if later.marker.is_none()
                    || earlier.scope.is_empty() == later.scope.is_empty()
                    || !earlier.meets(&later.kind, &later.identity, &later.scope)
                {
                    continue;
                }
                let refusal = self.refusal(&later.key, &earlier.method, &later.method);
                let (earlier_cfgs, later_cfgs) = (&earlier.cfgs, &later.cfgs);
                crossings.push(quote_spanned! {later.span=>
                    #(#earlier_cfgs)*
                    #(#later_cfgs)*
                    const _: () = ::core::panic!(#refusal);
                });
            }
        }
        quote! {
            #markers
            #(#crossings)*
        }
    }
}

/// The marker constant's name: a clause that reads as the rule in rustc's
/// `duplicate definitions with name` sentence.
///
/// The identity is written by the developer and may hold any character, so it
/// is folded to `[a-z0-9_]`. A fold that lost nothing is used as it is — it
/// never holds `__`, which an identity that is already a plain word does not
/// either — and a fold that lost something is followed by `__` and a stable hash
/// of the original, so two identities folding alike still name two constants.
fn marker_name(decorator: &str, kind: &str, identity: &str) -> String {
    let decorator: String = decorator
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let folded: String = identity
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '_' => c,
            _ => '_',
        })
        .collect();
    let lossless = !folded.is_empty() && folded == identity && !folded.contains("__");
    let identity = if lossless {
        folded
    } else {
        format!("{folded}__{:016x}", fnv1a(identity))
    };
    format!("__nestrs_{decorator}_dispatches_{kind}_{identity}_to_one_method")
}

/// FNV-1a, 64-bit — spelled here rather than borrowed from `std`, whose
/// hashers promise no stability across releases, so the name rustc prints for
/// one identity is the same on every toolchain.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use quote::{format_ident, quote};

    use super::*;

    #[test]
    fn a_plain_identity_reads_as_written() {
        assert_eq!(
            marker_name("#[messages]", "event", "ping"),
            "__nestrs_messages_dispatches_event_ping_to_one_method",
        );
    }

    #[test]
    fn identities_that_fold_alike_name_two_constants() {
        let dotted = marker_name("#[messages]", "event", "a.b");
        let dashed = marker_name("#[messages]", "event", "a-b");
        let plain = marker_name("#[messages]", "event", "a_b");
        assert_ne!(dotted, dashed);
        assert_ne!(dotted, plain);
        assert_ne!(dashed, plain);
        assert!(syn::parse_str::<syn::Ident>(&dotted).is_ok(), "{dotted}");
        assert!(
            syn::parse_str::<syn::Ident>(&marker_name("#[routes]", "route", "GET /users/:id"))
                .is_ok()
        );
    }

    #[test]
    fn two_unconditional_claims_are_refused_by_the_macro() {
        let mut keys = DispatchKeys::new("#[messages]", "the second would never run");
        let (first, second) = (format_ident!("first"), format_ident!("second"));
        let at: syn::Attribute = syn::parse_quote!(#[subscribe_message("ping")]);
        assert!(
            keys.declare(Collision::Marker, "event", "ping", "ping", &first, &[], &at)
                .is_ok()
        );
        let refusal = keys
            .declare(
                Collision::Marker,
                "event",
                "ping",
                "ping",
                &second,
                &[],
                &at,
            )
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
                Collision::Marker,
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
    fn a_conditional_claim_is_left_to_the_compiler() {
        let mut keys = DispatchKeys::new("#[messages]", "the second would never run");
        let at: syn::Attribute = syn::parse_quote!(#[subscribe_message("ping")]);
        let cfg = [quote!(#[cfg(all())])];
        assert!(
            keys.declare(
                Collision::Marker,
                "event",
                "ping",
                "ping",
                &format_ident!("a"),
                &[],
                &at
            )
            .is_ok()
        );
        assert!(
            keys.declare(
                Collision::Marker,
                "event",
                "ping",
                "ping",
                &format_ident!("b"),
                &cfg,
                &at
            )
            .is_ok()
        );
        let self_ty: syn::Type = syn::parse_quote!(Gateway);
        let markers = keys
            .markers(&self_ty, &syn::Generics::default())
            .to_string();
        assert_eq!(
            markers
                .matches("__nestrs_messages_dispatches_event_ping_to_one_method")
                .count(),
            2,
            "{markers}",
        );
    }
}
