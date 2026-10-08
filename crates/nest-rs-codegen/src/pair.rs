//! [`DecoratorPair`] — the two halves an edge decorator is written with, and the
//! one place their wrong-shape diagnostics are worded.
//!
//! One name on both a struct and its `impl` would give one rustdoc page for two
//! grammars, so an edge is a **pair**: the host on the struct, and on the impl a
//! sibling named for what it collects. Reaching for the wrong half names the
//! other:
//!
//! ```
//! # use quote::quote;
//! # fn main() -> syn::Result<()> {
//! # let input = quote! { struct Chat; };
//! let item = nest_rs_codegen::pair::WS.parse_host(input.into())?;       // in `#[gateway]`
//! # assert_eq!(item.ident, "Chat");
//! # let input = quote! { impl Chat {} };
//! let item = nest_rs_codegen::pair::WS.parse_operations(input.into())?; // in `#[messages]`
//! # assert!(item.items.is_empty());
//!
//! let wrong_half = nest_rs_codegen::pair::WS.parse_host(quote! { impl Chat {} });
//! assert!(wrong_half.is_err_and(|e| e.to_string().contains("#[messages]")));
//! # Ok(())
//! # }
//! ```
//!
//! **Every pair is declared here, and only here**: the constructors are
//! `pub(crate)`, so [`ALL`] is the whole population.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Item, ItemImpl, ItemStruct};

/// One edge's decorator pair: the vocabulary its two wrong-shape diagnostics are
/// built from.
pub struct DecoratorPair {
    /// The struct half as written, e.g. `"#[controller]"`.
    host: &'static str,
    /// How to name the struct the host half decorates, e.g. `"controller
    /// struct"`.
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
    /// half it belongs on. `#[use_guards]` is no attribute macro of its own, so
    /// unrefused it reaches rustc as `cannot find attribute`.
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
    /// processor, a scheduled-task host, an event-listener host.
    pub(crate) const fn on_provider(operations: &'static str, collects: &'static str) -> Self {
        Self {
            host: "#[injectable]",
            subject: "provider struct",
            operations,
            collects,
        }
    }

    /// Refuse an impl half whose host the container does not hold as a
    /// singleton under its own type, reading the
    /// `nest_rs_core::ProviderResidency` the struct's decorator recorded.
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
    /// Every edge host records `false`, explicitly, so a hand-written impl
    /// contradicting it is `E0119`.
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
    /// `declares` names what the host half takes.
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
    /// The item is parsed as an [`Item`] *before* the shape is judged, so a
    /// genuine syntax error reports itself.
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
    /// Returns the `impl` as written, and refuses a **trait** impl, whose
    /// methods the expansion would silently not collect.
    pub fn parse_operations(&self, input: TokenStream) -> syn::Result<ItemImpl> {
        match syn::parse2::<Item>(input)? {
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
    /// Keeping the item spares a cascade of `no method found` and `E0277`
    /// errors. `fallback` runs for an inherent impl alone.
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
        // By the last segment: a marker is consumed path-qualified as well as bare.
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
/// Free, not a [`DecoratorPair`] method: `#[injectable]` owns no pair — it is
/// the struct half every `on_provider` pair names — so it names the family.
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

/// The `impl ProviderResidency` a provider-building decorator emits:
/// `#[injectable]` (with `singleton` from its scope) and the four edge hosts
/// (always `false`, through [`DecoratorPair::host_residency`]).
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

    #[test]
    fn the_host_half_on_an_impl_names_the_operations_half() {
        for pair in ALL.iter().filter(|pair| !on_provider(pair)) {
            let msg = refusal(pair.parse_host(quote!(impl Host {})), pair.host());
            assert!(msg.contains(pair.operations()), "{}: {msg}", pair.host());
            assert!(msg.contains(pair.host()), "{}: {msg}", pair.host());
        }
    }

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
