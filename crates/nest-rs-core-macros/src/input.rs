//! `#[input]` — the wire-DTO shorthand. Carries `Serialize`, `Deserialize`,
//! `Validate` and `JsonSchema`, each routed through `nest_rs_core` with its own
//! `crate = ` override, plus `#[serde(deny_unknown_fields)]` so a payload
//! carrying an unknown field (e.g. `is_admin: true`) is rejected at parse time
//! instead of silently ignored. The derives are appended to any existing
//! `#[derive(...)]` so the user can still add `Debug`, `Clone`, etc.
//!
//! The routing is the point: a derive expands against the *call site's*
//! prelude, so without the overrides a DTO would oblige its crate to declare
//! `serde` / `validator` / `schemars` — the three lines this decorator exists
//! to absorb. It lives in the kernel rather than in HTTP because a wire type
//! crosses queues, gateways and tools too, and none of those should drag in the
//! HTTP stack to get a serde derive.
//!
//! `JsonSchema` is included because it is not optional in practice: `#[routes]`
//! documents every `Json<T>` / `Query<T>` argument in the OpenAPI document, so a
//! DTO without it fails to compile with a trait-bound error pointing at
//! `schema_of` rather than at the missing derive. Carrying it here is the
//! decorator doing its job; the alternative was every DTO repeating a derive the
//! shorthand exists to absorb.
//!
//! `Serialize` is there for the mirror reason: a wire DTO travels both ways, and
//! a response type rendered as `Json<T>` needs it. The derive list is public
//! contract — a reader who believes one is missing adds it by hand and hits
//! `E0119` — so the rustdoc on the re-exported attribute must name all four.
//! The unit test below holds the two in lockstep.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Item;

pub(crate) fn input(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args.into(), input.into()).into()
}

/// The expansion itself, over `proc_macro2` tokens so a unit test can call it —
/// a `proc_macro::TokenStream` cannot be built outside a real macro expansion.
/// Same split `#[crud]` uses for the same reason.
fn expand(args: TokenStream2, input: TokenStream2) -> TokenStream2 {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[input] takes no arguments",
        )
        .to_compile_error();
    }

    let item = match syn::parse2::<Item>(input) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };
    let Item::Struct(item) = item else {
        return syn::Error::new_spanned(item, "#[input] may only be applied to a struct")
            .to_compile_error();
    };

    // Routed through the surface crate, with each derive's `crate = ` override
    // set to the same path: a derive expands against the *call site's* prelude,
    // so without the override it would still emit `::serde::` internally and
    // oblige the developer to declare a crate `#[input]` exists to absorb.
    quote! {
        #[derive(
            ::nest_rs_core::serde::Serialize,
            ::nest_rs_core::serde::Deserialize,
            ::nest_rs_core::validator::Validate,
            ::nest_rs_core::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_core::serde", deny_unknown_fields)]
        #[validate(crate = ::nest_rs_core::validator)]
        #[schemars(crate = "::nest_rs_core::schemars")]
        #item
    }
}

#[cfg(test)]
mod tests {
    use syn::punctuated::Punctuated;
    use syn::{ItemStruct, Path, Token};

    use super::*;

    /// The derive names the expansion actually appends, read off the expanded
    /// tokens — not off this file's text, which would break on a reformat that
    /// changed nothing.
    fn emitted_derives() -> Vec<String> {
        let expanded: ItemStruct = syn::parse2(expand(
            TokenStream2::new(),
            quote! { struct CreateUser { name: String } },
        ))
        .expect("the expansion is still a struct");
        expanded
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("derive"))
            .flat_map(|attr| {
                attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
                    .expect("a derive list")
            })
            .map(|path| {
                path.segments
                    .last()
                    .expect("a derive name")
                    .ident
                    .to_string()
            })
            .collect()
    }

    /// The `///` block immediately above `item` — what docs.rs publishes.
    fn rustdoc_above(src: &str, item: &str) -> String {
        let (head, _) = src
            .split_once(item)
            .expect("the item is declared in `lib.rs` under this exact name");
        let mut lines: Vec<&str> = head
            .lines()
            .rev()
            // Step over `#[proc_macro_attribute]` and any other attribute
            // between the doc block and the function.
            .skip_while(|line| !line.trim_start().starts_with("///"))
            .take_while(|line| line.trim_start().starts_with("///"))
            .collect();
        lines.reverse();
        lines.join("\n")
    }

    /// R9-1: the expansion appends four derives, the published rustdoc listed
    /// three. `Serialize` was the missing one — precisely the derive that lets
    /// an `#[input]` DTO be *returned* as `Json<T>`, which is what the docs'
    /// own response snippets do. A reader trusting the list adds the derive by
    /// hand and gets `E0119` from a conflicting impl. `deny_unknown_fields`,
    /// the other half of the shorthand's contract, is pinned here too.
    #[test]
    fn the_public_rustdoc_names_everything_the_expansion_appends() {
        let derives = emitted_derives();
        assert_eq!(
            derives,
            ["Serialize", "Deserialize", "Validate", "JsonSchema"],
            "the expansion's derive list changed — update the rustdoc with it",
        );

        let doc = rustdoc_above(include_str!("lib.rs"), "pub fn input(");
        for named in derives
            .iter()
            .map(String::as_str)
            .chain(["deny_unknown_fields"])
        {
            assert!(
                doc.contains(named),
                "`#[input]`'s rustdoc must name `{named}`, the expansion adds it:\n{doc}",
            );
        }
    }

    #[test]
    fn the_expansion_rejects_what_it_cannot_shorten() {
        let not_a_struct = expand(TokenStream2::new(), quote! { enum Wire { A } }).to_string();
        assert!(
            not_a_struct.contains("only be applied to a struct"),
            "{not_a_struct}"
        );

        let with_args = expand(quote! { extra }, quote! { struct S {} }).to_string();
        assert!(with_args.contains("takes no arguments"), "{with_args}");
    }
}
