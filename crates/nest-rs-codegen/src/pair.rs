//! [`DecoratorPair`] — the two halves an edge decorator is written with, and the
//! one place their wrong-shape diagnostics are worded.
//!
//! An attribute macro is a single path in the macro namespace, so a name worn by
//! both a struct and its `impl` gives one rustdoc page for two argument grammars
//! and one symbol for go-to-definition. An edge is therefore written as a
//! **pair**: the host on the struct, and on the impl a sibling named for what it
//! collects. What makes the pair usable is the diagnostic — reaching for the
//! wrong half must say *which decorator the other shape wants*, never syn's
//! `expected struct`.
//!
//! Two shapes produce that message, and they are the same two sentences with the
//! halves swapped, so they live here rather than in nine macro crates:
//!
//! ```ignore
//! let item = nest_rs_codegen::pair::WS.parse_host(input.into())?;       // in `#[gateway]`
//! let item = nest_rs_codegen::pair::WS.parse_operations(input.into())?; // in `#[messages]`
//! ```
//!
//! **Every pair is declared here, and only here.** The fields are private and
//! the constructors `pub(crate)`, so a macro crate reads its pair from this
//! module and cannot declare one beside it: [`ALL`] is the whole population,
//! and this crate's unit tests run each wrong shape against every member of it.
//! Both halves read the *same* constant, which is what keeps the two sentences
//! from drifting into naming decorators that no longer exist. An impl-half
//! decorator whose struct half is the generic `#[injectable]` is built with
//! `on_provider` and gets the same treatment.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Item, ItemImpl, ItemStruct};

/// One edge's decorator pair: the vocabulary its two wrong-shape diagnostics are
/// built from.
///
/// Built only in this module — see the module doc — so the struct half and the
/// impl half cannot describe each other differently, and no pair exists outside
/// [`ALL`].
pub struct DecoratorPair {
    /// The struct half as written, e.g. `"#[controller]"`.
    host: &'static str,
    /// How to name the struct the host half decorates, e.g. `"controller
    /// struct"`. Read by *both* messages, so the two agree on what the item is.
    subject: &'static str,
    /// The impl half as written, e.g. `"#[routes]"`.
    operations: &'static str,
    /// What the impl half collects, as written, e.g. `"#[get] / #[post]"`.
    collects: &'static str,
}

/// The HTTP edge: `#[controller]` / `#[routes]`, and `#[crud]` beside them.
pub const HTTP: DecoratorPair = DecoratorPair::edge(
    "#[controller]",
    "controller struct",
    "#[routes]",
    "#[get] / #[post] / #[put] / #[patch] / #[delete]",
);

/// The GraphQL edge: `#[resolver]` / `#[operations]`, and `#[crud]` beside them.
pub const GRAPHQL: DecoratorPair = DecoratorPair::edge(
    "#[resolver]",
    "resolver struct",
    "#[operations]",
    "#[query] / #[mutation] / #[subscription] / #[entity] / #[field_resolver]",
);

/// The WebSocket edge: `#[gateway]` / `#[messages]`.
pub const WS: DecoratorPair = DecoratorPair::edge(
    "#[gateway]",
    "gateway struct",
    "#[messages]",
    "#[subscribe_message] / #[on_connect] / #[on_disconnect]",
);

/// The MCP edge: `#[mcp]` / `#[tools]`.
pub const MCP: DecoratorPair =
    DecoratorPair::edge("#[mcp]", "host struct", "#[tools]", "#[tool] / #[prompt]");

/// Lifecycle hooks on a provider.
pub const HOOKS: DecoratorPair = DecoratorPair::on_provider(
    "#[hooks]",
    "#[on_module_init] / #[on_application_bootstrap] / #[on_module_destroy] / \
     #[before_application_shutdown] / #[on_application_shutdown]",
);

/// A queue processor.
pub const PROCESSOR: DecoratorPair = DecoratorPair::on_provider("#[processor]", "#[process]");

/// Health indicators on a provider.
pub const INDICATORS: DecoratorPair =
    DecoratorPair::on_provider("#[indicators]", "#[liveness] / #[readiness] / #[startup]");

/// Scheduled tasks on a provider.
pub const SCHEDULED: DecoratorPair =
    DecoratorPair::on_provider("#[scheduled]", "#[every] / #[cron] / #[after]");

/// Event listeners on a provider.
pub const LISTENERS: DecoratorPair = DecoratorPair::on_provider("#[listeners]", "#[on_event]");

/// Every pair the framework declares.
pub const ALL: [&DecoratorPair; 9] = [
    &HTTP,
    &GRAPHQL,
    &WS,
    &MCP,
    &HOOKS,
    &PROCESSOR,
    &INDICATORS,
    &SCHEDULED,
    &LISTENERS,
];

impl DecoratorPair {
    /// An edge's pair: its own struct decorator and the impl half beside it.
    pub(crate) const fn edge(
        host: &'static str,
        subject: &'static str,
        operations: &'static str,
        collects: &'static str,
    ) -> Self {
        Self {
            host,
            subject,
            operations,
            collects,
        }
    }

    /// The struct half as written, e.g. `"#[controller]"`.
    pub const fn host(&self) -> &'static str {
        self.host
    }

    /// The impl half as written, e.g. `"#[routes]"`.
    pub const fn operations(&self) -> &'static str {
        self.operations
    }

    /// What the impl half collects, as written, e.g. `"#[get] / #[post]"`.
    pub const fn collects(&self) -> &'static str {
        self.collects
    }
}

impl DecoratorPair {
    /// Refuse a host-scope layer written on the impl half, naming the struct
    /// half it belongs on.
    ///
    /// **The sentence must not enumerate the siblings**, and that is the whole
    /// reason this is here. Two edges worded it themselves and had already
    /// drifted in *content*, not merely in phrasing: MCP's named
    /// `#[controller]`, `#[resolver]` and `#[gateway]`, GraphQL's named
    /// `#[controller]` and `#[gateway]` — so a resolver author was told the
    /// uniformity spanned two edges when it spanned four, and every edge added
    /// later would have inherited a list that was wrong the day it landed. The
    /// pair already knows its own two nouns; a reader needs those, not a roll
    /// call.
    ///
    /// The other two edges said nothing at all. `#[use_guards]` is not a
    /// standalone attribute macro anywhere in the tree — it exists only as text
    /// a host decorator consumes — so on a `#[routes]` or `#[messages]` impl it
    /// reached rustc as `cannot find attribute `use_guards` in this scope`,
    /// which is verbatim the failure `attrs.rs` cites as the reason
    /// `HTTP_ONLY_LAYERS` is a constant: no transport named, no reason, no
    /// remedy.
    pub fn reject_host_layers(&self, attrs: &[syn::Attribute]) -> syn::Result<()> {
        const HOST_SCOPE: [&str; 2] = ["use_guards", "force_guards"];
        for attr in attrs {
            let Some(name) = HOST_SCOPE.iter().find(|name| attr.path().is_ident(name)) else {
                continue;
            };
            return Err(syn::Error::new_spanned(
                attr,
                format!(
                    "`#[{name}(...)]` belongs on the {subject} beside `{host}`, not on \
                     this `{operations}` block — the struct half declares the host's \
                     access posture, and this half declares its {collects}. A layer \
                     scoped to one operation goes on that operation's method.",
                    subject = self.subject,
                    host = self.host,
                    operations = self.operations,
                    collects = self.collects,
                ),
            ));
        }
        Ok(())
    }

    /// A pair whose struct half is the generic `#[injectable]` — a queue
    /// processor, a scheduled-task host, an event-listener host. There is no
    /// edge-specific struct decorator to name, but reaching for the impl half on
    /// a struct still deserves better than `expected impl`.
    pub(crate) const fn on_provider(operations: &'static str, collects: &'static str) -> Self {
        Self {
            host: "#[injectable]",
            subject: "provider struct",
            operations,
            collects,
        }
    }

    /// The third wrong-shape refusal, and the one only rustc can deliver: the
    /// impl half sits on an `impl` block all right, but the container will not
    /// hold the type it collects for.
    ///
    /// An `on_provider` half resolves its host with
    /// `Container::get::<Host>()`, outside any request — which answers only for
    /// a singleton stored under its own type. An edge host registers metadata;
    /// a `scope = request` provider registers a factory; a `scope = transient`
    /// one hands back a throwaway whose effects are dropped. A macro cannot see
    /// the struct's decorator from the impl block, so the refusal reads the fact
    /// the *struct's* decorator recorded: `nest_rs_core::ProviderResidency`.
    ///
    /// Two diagnostics fall out, and both are the framework's own words: a type
    /// no decorator built has no impl at all and gets the trait's
    /// `#[diagnostic::on_unimplemented]`; a type whose decorator recorded
    /// `SINGLETON = false` fails this `const` assertion. **Reading a stated fact
    /// rather than requiring a marker is the whole point** — a marker is absent
    /// for the shapes it refuses, and absence is fillable by hand, which is how
    /// a transient host once slipped through the very bound meant to refuse it.
    ///
    /// Every `on_provider` half emits this after a successful
    /// [`parse_operations`](Self::parse_operations); the edge pairs must not —
    /// their hosts are what it refuses.
    pub fn provider_host_check(&self, self_ty: &syn::Type) -> TokenStream {
        debug_assert!(
            self.host == "#[injectable]",
            "only an on_provider pair reads a host's residency",
        );
        quote! {
            const _: () = ::core::assert!(
                <#self_ty as ::nest_rs_core::ProviderResidency>::SINGLETON,
                "a provider-hosted decorator (#[hooks], #[scheduled], #[listeners], \
                 #[indicators], #[processor]) resolves its host with Container::get::<Self>(), \
                 outside any request, so the host must be a provider the container holds under \
                 its own type for the app's lifetime. This one is not: an edge host \
                 (#[controller], #[gateway], #[resolver], #[mcp]) is built at mount, \
                 `scope = request` builds one per request, and `scope = transient` would hand \
                 it a throwaway whose effects are dropped. Move these methods to a plain \
                 #[injectable] provider.",
            );
        }
    }

    /// Emit the struct half's record of what the container will hold under this
    /// type — the fact [`provider_host_check`](Self::provider_host_check)
    /// reads, written by the decorator that builds the provider.
    ///
    /// Every edge host records `false`: `#[controller]`, `#[gateway]`,
    /// `#[resolver]` and `#[mcp]` register *metadata*, and the instance is
    /// built at mount. It is written rather than omitted so that contradicting
    /// it is `E0119` — a marker that is merely absent for the shapes it refuses
    /// can be filled in by hand, which is how a `scope = transient` host once
    /// slipped through the bound meant to refuse it.
    ///
    /// Here rather than in each `*-macros` crate for the reason the four copies
    /// demonstrated: the edge *form* is open, so the path a fifth edge follows
    /// is whatever the other four did — and a forgotten copy reopens that hole
    /// silently, since a missing impl falls back to the trait's
    /// `on_unimplemented` note, which reads plausibly.
    pub fn host_residency(&self, name: &syn::Ident, generics: &syn::Generics) -> TokenStream {
        debug_assert!(
            self.host != "#[injectable]",
            "a provider-hosted pair records residency through `#[injectable]`, not its impl half",
        );
        provider_residency(name, generics, false)
    }

    /// Refuse an argument list on the **impl** half, naming what does declare
    /// the thing the developer probably reached for.
    ///
    /// The impl half collects; it declares nothing. Every edge owes the same
    /// sentence, and every edge was writing its own — nine copies, differing in
    /// wording and in span mechanism. Refusals are shared, not per key:
    /// per-key refusals multiply with the matrix, and what multiplies is what
    /// gets skipped. The pair already carries both nouns the sentence needs.
    ///
    /// `declares` names what the host half takes, so the remedy points at the
    /// line above rather than merely refusing.
    pub fn reject_args(&self, args: &TokenStream, declares: &str) -> syn::Result<()> {
        if args.is_empty() {
            return Ok(());
        }
        let Self {
            host,
            operations,
            collects,
            ..
        } = self;
        Err(syn::Error::new_spanned(
            args,
            format!(
                "{operations} takes no arguments; {declares} {host}, and each operation by \
                 {collects}",
            ),
        ))
    }

    /// Parse the **struct** half's input, naming the impl half when the
    /// developer decorated an `impl` block instead.
    ///
    /// The item is parsed as an [`Item`] *before* the shape is judged: a struct
    /// with a genuine syntax error must report that error, not "you wanted the
    /// other decorator" — which is the failure mode this whole indirection
    /// exists to avoid.
    pub fn parse_host(&self, input: TokenStream) -> syn::Result<ItemStruct> {
        match syn::parse2::<Item>(input)? {
            Item::Struct(item) => Ok(item),
            other => Err(syn::Error::new_spanned(
                other,
                format!(
                    "{host} decorates the {subject} — its {collects} methods go under \
                     {operations} on the impl block",
                    host = self.host,
                    subject = self.subject,
                    collects = self.collects,
                    operations = self.operations,
                ),
            )),
        }
    }

    /// Parse the **impl** half's input, naming the struct half when the
    /// developer decorated the struct instead.
    ///
    /// Returns the `impl` as written, and refuses a **trait** impl for every
    /// pair: it parses as an `Item::Impl` like any other, so the shape check
    /// alone waves it through and the expansion collects nothing. Eight of the
    /// nine halves had no answer to that shape at all and one wrote its own,
    /// which is the drift this const exists to prevent.
    pub fn parse_operations(&self, input: TokenStream) -> syn::Result<ItemImpl> {
        match syn::parse2::<Item>(input)? {
            // A trait impl parses as an `Item::Impl` like any other, so the
            // shape check above waves it through — and the expansion then
            // collects nothing, because the methods it looks for are the
            // trait's. The half is accepted, the route or the tick declared
            // there never exists, and nothing says so. Refused here rather than
            // per decorator: eight of nine had no answer at all, the ninth
            // worded its own, and one sentence is what keeps the nine from
            // drifting apart.
            Item::Impl(item) if item.trait_.is_some() => {
                #[expect(
                    clippy::expect_used,
                    reason = "a compile-time invariant of the parse above; a panic in a proc macro is a compile error"
                )]
                let (path, _) = item.trait_.as_ref().expect("just matched as some");
                let subject = item.self_ty.to_token_stream();
                Err(syn::Error::new_spanned(
                    path,
                    format!(
                        "{operations} decorates the inherent impl holding the {collects} \
                         methods — this one implements `{implemented}` for `{subject}`, whose \
                         methods answer to that trait. Move it to `impl {subject} {{ … }}`",
                        operations = self.operations,
                        collects = self.collects,
                        implemented = path.to_token_stream(),
                    ),
                ))
            }
            Item::Impl(item) => Ok(item),
            other => Err(syn::Error::new_spanned(
                other,
                format!(
                    "{operations} decorates the impl block holding the {collects} methods — \
                     the {subject} itself takes {host}",
                    operations = self.operations,
                    collects = self.collects,
                    subject = self.subject,
                    host = self.host,
                ),
            )),
        }
    }
}

/// The attributes every impl half consumes off a method or its block, whatever
/// else the half takes — the layers, the posture, and the two HTTP-only keys a
/// half refuses by name elsewhere.
const CONSUMED_BY_EVERY_HALF: [&str; 11] = [
    "use_guards",
    "force_guards",
    "use_interceptors",
    "use_filters",
    "use_pipes",
    "use_exception_filters",
    "public",
    "authorize",
    "version",
    "api",
    "meta",
];

impl DecoratorPair {
    /// What an impl half emits: `expansion` when it expanded, and when it refused
    /// — nothing but `compile_error!`s — those errors **beside the item as the
    /// developer wrote it**, with the attributes the half would have consumed
    /// taken off (`helpers`, plus the layers and the posture every half consumes)
    /// and the trait impls a host is required to carry filled in by `fallback`.
    ///
    /// A refusal used to drop the whole `impl`, so the one real error arrived
    /// under a cascade it caused: every method became `no method found`, every
    /// import `unused`, and the host failed `Discoverable` or `McpHost` at the
    /// module listing it — `E0277` blamed on a line that is right. The item stays,
    /// so what rustc reports beside the refusal is only what is wrong in the
    /// developer's own code. `fallback` runs for an inherent impl alone: a trait
    /// impl is itself the refusal, and a second impl of the host's traits would
    /// only add a conflict.
    pub fn keep_item_on_refusal(
        &self,
        input: TokenStream,
        expansion: TokenStream,
        helpers: &[&str],
        fallback: impl FnOnce(&ItemImpl) -> TokenStream,
    ) -> TokenStream {
        if !only_compile_errors(&expansion) {
            return expansion;
        }
        let Ok(item) = syn::parse2::<Item>(input.clone()) else {
            return expansion;
        };
        let Item::Impl(mut item) = item else {
            return quote!(#expansion #input);
        };
        // By the last segment: an exported marker (`#[nest_rs::http::http_code]`)
        // is consumed path-qualified as well as bare, and one left on the item
        // would add its own "unread" refusal to the one being reported.
        let consumed = |attr: &syn::Attribute| {
            let Some(last) = attr.path().segments.last() else {
                return false;
            };
            helpers
                .iter()
                .chain(CONSUMED_BY_EVERY_HALF.iter())
                .any(|name| last.ident == name)
        };
        item.attrs.retain(|attr| !consumed(attr));
        for entry in &mut item.items {
            let syn::ImplItem::Fn(method) = entry else {
                continue;
            };
            method.attrs.retain(|attr| !consumed(attr));
            for input in &mut method.sig.inputs {
                if let syn::FnArg::Typed(typed) = input {
                    typed.attrs.retain(|attr| !consumed(attr));
                }
            }
        }
        let fallback = item.trait_.is_none().then(|| fallback(&item));
        quote!(#expansion #item #fallback)
    }
}

/// Whether `tokens` hold `compile_error!` invocations and nothing else — the
/// shape `syn::Error::to_compile_error` writes, one error or several combined.
///
/// Read structurally rather than flagged by the caller: a half has dozens of
/// refusal sites, and an expansion that expanded always holds the item it was
/// given, so a stream of nothing but errors is a refusal by construction.
fn only_compile_errors(tokens: &TokenStream) -> bool {
    use proc_macro2::TokenTree;
    let mut named_the_macro = false;
    let mut any = false;
    for token in tokens.clone() {
        match token {
            TokenTree::Ident(ident) if ident == "core" || ident == "std" => {}
            TokenTree::Ident(ident) if ident == "compile_error" => named_the_macro = true,
            TokenTree::Punct(punct) if matches!(punct.as_char(), ':' | '!' | ';') => {}
            TokenTree::Group(_) if named_the_macro => {
                named_the_macro = false;
                any = true;
            }
            _ => return false,
        }
    }
    any && !named_the_macro
}

/// Parse `#[injectable]`'s input, naming the impl halves when the developer
/// decorated the impl block instead.
///
/// Free rather than a [`DecoratorPair`] method for the same reason as
/// [`provider_residency`]: `#[injectable]` owns no pair — it *is* the generic
/// struct half all five `on_provider` pairs name. So it is the one host whose
/// refusal cannot name *the* sibling, and names the family instead; which of
/// the five the developer wanted is theirs to know, and all five are one
/// sentence away.
///
/// Worded here rather than in `#[injectable]`'s own crate because that is the
/// whole point of this module: the five pairs already say "the struct itself
/// takes `#[injectable]`", and the sentence coming back the other way has to
/// agree with them. It answered `expected struct` until this existed — the one
/// phrasing that is the defect.
pub fn parse_provider_host(input: TokenStream) -> syn::Result<ItemStruct> {
    match syn::parse2::<Item>(input)? {
        Item::Struct(item) => Ok(item),
        other => Err(syn::Error::new_spanned(
            other,
            "#[injectable] decorates the provider struct — the impl block holding its \
             methods takes the decorator named for what it collects: #[processor], \
             #[scheduled], #[listeners], #[indicators] or #[hooks]",
        )),
    }
}

/// The `impl ProviderResidency` a provider-building decorator emits, spelled
/// once for all five: `#[injectable]` (with `singleton` from its scope) and the
/// four edge hosts (always `false`, through
/// [`DecoratorPair::host_residency`]).
///
/// Free rather than a method because `#[injectable]` owns no pair — it *is* the
/// generic struct half every `on_provider` pair names.
pub fn provider_residency(
    name: &syn::Ident,
    generics: &syn::Generics,
    singleton: bool,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    quote! {
        impl #impl_generics ::nest_rs_core::ProviderResidency
            for #name #ty_generics #where_clause
        {
            const SINGLETON: bool = #singleton;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    /// `syn`'s item types carry no `Debug` (the `extra-traits` feature is off), so
    /// `expect_err` is unavailable; `err().expect(..)` bounds nothing on the `Ok`
    /// type and asserts the same thing.
    fn refusal<T>(result: syn::Result<T>, what: &str) -> String {
        result.err().expect(what).to_string()
    }

    fn on_provider(pair: &DecoratorPair) -> bool {
        pair.host() == "#[injectable]"
    }

    // The whole point of a pair: each half's refusal names the *other* half, so
    // the compiler tells the reader which decorator it is looking at. Run over
    // every pair the framework declares, since `ALL` is the population.
    #[test]
    fn the_host_half_on_an_impl_names_the_operations_half() {
        for pair in ALL.iter().filter(|pair| !on_provider(pair)) {
            let msg = refusal(pair.parse_host(quote!(impl Host {})), pair.host());
            assert!(msg.contains(pair.operations()), "{}: {msg}", pair.host());
            assert!(msg.contains(pair.host()), "{}: {msg}", pair.host());
        }
    }

    /// `#[injectable]` owns no pair, so its refusal names the family — every
    /// impl half whose struct half it is.
    #[test]
    fn injectable_on_an_impl_names_every_provider_half() {
        let msg = refusal(parse_provider_host(quote!(impl Host {})), "#[injectable]");
        for pair in ALL.iter().filter(|pair| on_provider(pair)) {
            assert!(
                msg.contains(pair.operations()),
                "{}: {msg}",
                pair.operations()
            );
        }
    }

    #[test]
    fn the_operations_half_on_a_struct_names_the_host_half() {
        for pair in ALL {
            let msg = refusal(
                pair.parse_operations(quote!(
                    struct Host;
                )),
                pair.operations(),
            );
            assert!(msg.contains(pair.host()), "{}: {msg}", pair.operations());
            assert!(msg.contains(pair.subject), "{}: {msg}", pair.operations());
        }
    }

    #[test]
    fn the_operations_half_on_a_trait_impl_names_the_inherent_impl() {
        for pair in ALL {
            let msg = refusal(
                pair.parse_operations(quote!(impl Display for Host {})),
                pair.operations(),
            );
            assert!(
                msg.contains(pair.operations()),
                "{}: {msg}",
                pair.operations()
            );
            assert!(msg.contains("impl Host"), "{}: {msg}", pair.operations());
        }
    }

    #[test]
    fn arguments_on_the_operations_half_name_the_host_half() {
        for pair in ALL {
            let msg = refusal(
                pair.reject_args(&quote!(path = "/x"), "declare it on"),
                pair.operations(),
            );
            assert!(
                msg.contains(pair.operations()),
                "{}: {msg}",
                pair.operations()
            );
            assert!(msg.contains(pair.host()), "{}: {msg}", pair.operations());
        }
    }

    #[test]
    fn a_host_layer_on_the_operations_half_names_the_host_half() {
        let attrs: Vec<syn::Attribute> = vec![syn::parse_quote!(#[use_guards(AuthGuard)])];
        for pair in ALL {
            let msg = refusal(pair.reject_host_layers(&attrs), pair.operations());
            assert!(msg.contains(pair.host()), "{}: {msg}", pair.operations());
        }
    }

    /// What a macro crate reads off a pair is what the pair declares.
    #[test]
    fn the_accessors_read_the_declared_halves() {
        assert_eq!(
            (GRAPHQL.host(), GRAPHQL.operations()),
            ("#[resolver]", "#[operations]")
        );
        for pair in ALL {
            assert!(pair.collects().starts_with("#["), "{}", pair.collects());
        }
    }

    /// One decorator, one pair: an impl half named twice would give two pairs
    /// one rustdoc page.
    #[test]
    fn every_pair_has_its_own_halves() {
        for (i, pair) in ALL.iter().enumerate() {
            for other in &ALL[i + 1..] {
                assert_ne!(pair.operations(), other.operations);
                assert!(
                    on_provider(pair) || pair.host() != other.host,
                    "{}",
                    pair.host()
                );
            }
        }
    }

    // Neither message may swallow a real syntax error: a struct that does not
    // parse has to report *that*, or the indirection has made diagnostics worse
    // rather than better.
    #[test]
    fn a_genuine_syntax_error_is_reported_as_itself() {
        let msg = refusal(
            WS.parse_host(quote!(struct Gateway { : })),
            "malformed input must fail",
        );
        assert!(
            !msg.contains("#[messages]"),
            "a syntax error must not be reported as a wrong-shape hint: {msg}"
        );
    }

    /// A refusal keeps the item, minus what the half consumes; an expansion is
    /// left alone.
    #[test]
    fn a_refusal_keeps_the_item_without_the_consumed_attributes() {
        let input = quote! {
            impl Gateway {
                #[on_x]
                #[public]
                #[doc = "kept"]
                async fn run(&self) {}
            }
        };
        let refusal = syn::Error::new(proc_macro2::Span::call_site(), "no").to_compile_error();
        let kept = WS
            .keep_item_on_refusal(
                input.clone(),
                refusal.clone(),
                &["on_x"],
                |_| quote!(impl Fallback for Gateway {}),
            )
            .to_string();
        let expected = quote! {
            #refusal
            impl Gateway {
                #[doc = "kept"]
                async fn run(&self) {}
            }
            impl Fallback for Gateway {}
        };
        assert_eq!(kept, expected.to_string());

        let expansion = quote!(impl Gateway {} const _: () = (););
        assert_eq!(
            WS.keep_item_on_refusal(input, expansion.clone(), &[], |_| quote!())
                .to_string(),
            expansion.to_string(),
        );
    }

    #[test]
    fn the_right_shape_parses_through() {
        for pair in ALL {
            assert!(
                pair.parse_host(quote!(
                    struct Host;
                ))
                .is_ok()
            );
            assert!(pair.parse_operations(quote!(impl Host {})).is_ok());
            assert!(pair.reject_args(&quote!(), "").is_ok());
        }
    }
}
