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
    /// Returns the `impl` as written; a half that additionally refuses a *trait*
    /// impl (`#[tools]`) checks that itself — the distinction is its own, not
    /// every pair's.
    pub fn parse_operations(&self, input: TokenStream) -> syn::Result<ItemImpl> {
        match syn::parse2::<Item>(input)? {
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
