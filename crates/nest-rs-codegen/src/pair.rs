//! [`DecoratorPair`] — the two halves an edge decorator is written with, and the
//! one place their wrong-shape diagnostics are worded.
//!
//! An attribute macro is a single path in the macro namespace, so a name worn by
//! both a struct and its `impl` gives one rustdoc page for two argument grammars
//! and one symbol for go-to-definition. `CLAUDE.md` therefore fixes an edge as a
//! **pair**: the host on the struct, and on the impl a sibling named for what it
//! collects. What makes the pair usable is the diagnostic — reaching for the
//! wrong half must say *which decorator the other shape wants*, never syn's
//! `expected struct`.
//!
//! Two shapes produce that message, and they are the same two sentences with the
//! halves swapped, so they live here rather than in six macro crates:
//!
//! ```ignore
//! const PAIR: DecoratorPair = DecoratorPair {
//!     host: "#[gateway]",
//!     subject: "gateway struct",
//!     operations: "#[messages]",
//!     collects: "#[subscribe_message] / #[on_connect] / #[on_disconnect]",
//! };
//!
//! let item = PAIR.parse_host(input.into())?;       // in `#[gateway]`
//! let item = PAIR.parse_operations(input.into())?; // in `#[messages]`
//! ```
//!
//! Both halves read the *same* constant, which is what keeps the two sentences
//! from drifting into naming decorators that no longer exist. An impl-half
//! decorator whose struct half is the generic `#[injectable]` uses
//! [`DecoratorPair::on_provider`] and gets the same treatment.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Item, ItemImpl, ItemStruct};

/// One edge's decorator pair: the vocabulary its two wrong-shape diagnostics are
/// built from.
///
/// Declared as a `const` in the macro crate that owns the pair, so the struct
/// half and the impl half cannot describe each other differently. A plain struct
/// literal rather than a builder: Rust already refuses a half-named pair, so
/// there is nothing for a builder to enforce.
pub struct DecoratorPair {
    /// The struct half as written, e.g. `"#[controller]"`.
    pub host: &'static str,
    /// How to name the struct the host half decorates, e.g. `"controller
    /// struct"`. Read by *both* messages, so the two agree on what the item is.
    pub subject: &'static str,
    /// The impl half as written, e.g. `"#[routes]"`.
    pub operations: &'static str,
    /// What the impl half collects, as written, e.g. `"#[get] / #[post]"`.
    pub collects: &'static str,
}

impl DecoratorPair {
    /// A pair whose struct half is the generic `#[injectable]` — a queue
    /// processor, a scheduled-task host, an event-listener host. There is no
    /// edge-specific struct decorator to name, but reaching for the impl half on
    /// a struct still deserves better than `expected impl`.
    pub const fn on_provider(operations: &'static str, collects: &'static str) -> Self {
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
    /// An [`on_provider`](Self::on_provider) half resolves its host with
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
    /// wording and in span mechanism, which is what CLAUDE.md means by
    /// "refusals are shared, not per key: per-key refusals multiply with the
    /// matrix, and what multiplies is what gets skipped". The pair already
    /// carries both nouns the sentence needs.
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
/// phrasing `CLAUDE.md` names as the defect.
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

    const PAIR: DecoratorPair = DecoratorPair {
        host: "#[gateway]",
        subject: "gateway struct",
        operations: "#[messages]",
        collects: "#[on_x]",
    };

    /// `syn`'s item types carry no `Debug` (the `extra-traits` feature is off), so
    /// `expect_err` is unavailable; `err().expect(..)` bounds nothing on the `Ok`
    /// type and asserts the same thing.
    fn refusal<T>(result: syn::Result<T>, what: &str) -> String {
        result.err().expect(what).to_string()
    }

    // The whole point of the pair: each half's refusal names the *other* half,
    // so the compiler tells the reader which decorator it is looking at.
    #[test]
    fn the_host_half_on_an_impl_names_the_operations_half() {
        let msg = refusal(
            PAIR.parse_host(quote!(impl Gateway {})),
            "an impl on #[gateway] must be refused",
        );
        assert!(msg.contains("#[messages]"), "{msg}");
        assert!(msg.contains("#[gateway]"), "{msg}");
    }

    #[test]
    fn the_operations_half_on_a_struct_names_the_host_half() {
        let msg = refusal(
            PAIR.parse_operations(quote!(
                struct Gateway;
            )),
            "a struct on #[messages] must be refused",
        );
        assert!(msg.contains("#[gateway]"), "{msg}");
        assert!(msg.contains("gateway struct"), "{msg}");
    }

    // Neither message may swallow a real syntax error: a struct that does not
    // parse has to report *that*, or the indirection has made diagnostics worse
    // rather than better.
    #[test]
    fn a_genuine_syntax_error_is_reported_as_itself() {
        let msg = refusal(
            PAIR.parse_host(quote!(struct Gateway { : })),
            "malformed input must fail",
        );
        assert!(
            !msg.contains("#[messages]"),
            "a syntax error must not be reported as a wrong-shape hint: {msg}"
        );
    }

    #[test]
    fn a_provider_pair_names_injectable_as_the_struct_half() {
        const PROCESSOR: DecoratorPair = DecoratorPair::on_provider("#[processor]", "#[process]");
        let msg = refusal(
            PROCESSOR.parse_operations(quote!(
                struct Worker;
            )),
            "a struct on #[processor] must be refused",
        );
        assert!(msg.contains("#[injectable]"), "{msg}");
        assert!(msg.contains("#[process]"), "{msg}");
    }

    #[test]
    fn the_right_shape_parses_through() {
        assert!(
            PAIR.parse_host(quote!(
                struct Gateway;
            ))
            .is_ok()
        );
        assert!(PAIR.parse_operations(quote!(impl Gateway {})).is_ok());
    }
}
