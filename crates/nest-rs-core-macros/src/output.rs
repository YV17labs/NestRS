//! `#[output]` — the reply-DTO shorthand: `Serialize` and `JsonSchema` routed
//! through `nest_rs_core`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Item;

pub(crate) fn output(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args.into(), input.into()).into()
}

/// Over `proc_macro2` tokens so a unit test can call it: a `proc_macro::TokenStream`
/// cannot be built outside a real macro expansion.
fn expand(args: TokenStream2, input: TokenStream2) -> TokenStream2 {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[output] takes no arguments",
        )
        .to_compile_error();
    }

    let item = match syn::parse2::<Item>(input) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };
    if !matches!(item, Item::Struct(_) | Item::Enum(_)) {
        let shape = match &item {
            Item::Union(_) => "a union",
            Item::Type(_) => "a type alias",
            Item::Trait(_) => "a trait",
            Item::Fn(_) => "a function",
            Item::Impl(_) => "an impl block",
            Item::Mod(_) => "a module",
            _ => "neither a struct nor an enum",
        };
        return syn::Error::new_spanned(
            &item,
            format!(
                "#[output] takes a struct or an enum, and this is {shape}: it derives \
                 `Serialize` and `JsonSchema`, which describe a value an edge sends"
            ),
        )
        .to_compile_error();
    }

    // As `#[input]`'s: each derive relocated, so the DTO's crate declares none
    // of serde or schemars.
    quote! {
        #[derive(
            ::nest_rs_core::__private::serde::Serialize,
            ::nest_rs_core::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_core::__private::serde")]
        #[schemars(crate = "::nest_rs_core::__private::schemars")]
        #item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expansion_takes_a_struct_or_an_enum_and_nothing_else() {
        for item in [
            quote! { struct Person { name: String } },
            quote! { struct Slug(String); },
            quote! { enum State { Ready } },
        ] {
            let expanded = expand(TokenStream2::new(), item).to_string();
            assert!(
                expanded.contains("JsonSchema") && !expanded.contains("compile_error"),
                "{expanded}"
            );
            assert!(
                !expanded.contains("Deserialize") && !expanded.contains("Validate"),
                "a reply is never decoded: {expanded}"
            );
        }

        let refused = expand(TokenStream2::new(), quote! { union U { a: u32 } }).to_string();
        assert!(
            refused.contains("takes a struct or an enum, and this is a union"),
            "{refused}"
        );
        let with_args = expand(quote! { extra }, quote! { struct S {} }).to_string();
        assert!(with_args.contains("takes no arguments"), "{with_args}");
    }
}
