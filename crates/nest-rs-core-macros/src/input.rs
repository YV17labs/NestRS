//! `#[input]` — the wire-DTO shorthand: `Serialize`, `Deserialize`, `Validate`
//! and `JsonSchema` routed through `nest_rs_core`, plus
//! `#[serde(deny_unknown_fields)]`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Item;

pub(crate) fn input(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args.into(), input.into()).into()
}

/// The limit is `validator`'s: its `Validate` derive takes only a struct with
/// named fields; the other three derives accept an enum.
fn refuse_shape(item: &impl quote::ToTokens, shape: &str) -> syn::Error {
    syn::Error::new_spanned(
        item,
        format!(
            "#[input] takes a struct with named fields, and this is {shape}. \
             `#[input]` bundles `validator::Validate`, whose derive supports only \
             that shape (`Serialize`, `Deserialize` and `JsonSchema` do not mind). \
             For a tagged union, put `#[input]` on each variant's payload struct \
             and derive the wire traits on the enum itself; for a newtype, give \
             the field a name.",
        ),
    )
}

/// Over `proc_macro2` tokens so a unit test can call it: a `proc_macro::TokenStream`
/// cannot be built outside a real macro expansion.
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
        let shape = match &item {
            Item::Enum(_) => "an enum",
            Item::Union(_) => "a union",
            Item::Type(_) => "a type alias",
            Item::Trait(_) => "a trait",
            Item::Fn(_) => "a function",
            Item::Impl(_) => "an impl block",
            Item::Mod(_) => "a module",
            _ => "not a struct",
        };
        return refuse_shape(&item, shape).to_compile_error();
    };
    // Left to `validator`, a tuple struct is refused at a `#[derive(...)]` line
    // the developer never wrote, with no mention of `#[input]`.
    if !matches!(item.fields, syn::Fields::Named(_)) {
        let shape = match item.fields {
            syn::Fields::Unnamed(_) => "a tuple struct",
            _ => "a unit struct",
        };
        return refuse_shape(&item, shape).to_compile_error();
    }

    // A derive expands against the call site's prelude: without each `crate = `
    // override it emits `::serde::` and the DTO's crate must declare serde.
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
    use super::*;

    #[test]
    fn the_expansion_rejects_what_it_cannot_shorten() {
        for (item, named) in [
            (quote! { enum Wire { A } }, "this is an enum"),
            (quote! { union U { a: u32 } }, "this is a union"),
            (quote! { struct Slug(String); }, "this is a tuple struct"),
            (quote! { struct Marker; }, "this is a unit struct"),
        ] {
            let refused = expand(TokenStream2::new(), item).to_string();
            assert!(
                refused.contains("struct with named fields"),
                "the sentence states the shape it takes: {refused}"
            );
            assert!(
                refused.contains("validator::Validate"),
                "the sentence names the derive whose limit this is: {refused}"
            );
            assert!(
                refused.contains(named),
                "the sentence names the shape written: {refused}"
            );
        }

        let with_args = expand(quote! { extra }, quote! { struct S {} }).to_string();
        assert!(with_args.contains("takes no arguments"), "{with_args}");
    }
}
