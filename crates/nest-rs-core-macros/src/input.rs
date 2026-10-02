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
//! `E0119` — so the rustdoc on the re-exported attribute names all four, and
//! `nest-rs-http`'s `input` suite holds what each one does.

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
/// One sentence for every shape `#[input]` cannot carry, naming the fact rather
/// than the rule.
///
/// The fact is checkable and it is `validator`'s, not ours: its `Validate`
/// derive answers any other shape with *"Expected struct with named fields"*.
/// The other three derives in the bundle — `Serialize`, `Deserialize`,
/// `JsonSchema` — all accept an enum happily, so the limit is one derive's and
/// the sentence says which. That distinction is what keeps this a refusal
/// rather than a guess: a reader can verify it, and if `validator` ever grows
/// enum support the sentence is what tells them this became "not yet".
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
        // Name the shape the developer actually wrote. "an enum, union or other
        // item" makes the reader check which of three they hit; the compiler
        // already knows.
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
    // A tuple or unit struct reached the derives and was refused *inside* the
    // expansion, by `validator`, pointing at a `#[derive(...)]` line the
    // developer never wrote — "Unsupported shape `one unnamed field`" with no
    // mention of `#[input]`. The shape is knowable here, so the refusal belongs
    // here: a refusal lands at the earliest site that can see the fact.
    if !matches!(item.fields, syn::Fields::Named(_)) {
        let shape = match item.fields {
            syn::Fields::Unnamed(_) => "a tuple struct",
            _ => "a unit struct",
        };
        return refuse_shape(&item, shape).to_compile_error();
    }

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
    use super::*;

    #[test]
    fn the_expansion_rejects_what_it_cannot_shorten() {
        // Every shape the bundle cannot carry is refused *here*, naming the
        // derive whose limit it is — a tuple struct used to reach `validator`
        // and be refused inside the expansion, at a `#[derive(...)]` line the
        // developer never wrote.
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
