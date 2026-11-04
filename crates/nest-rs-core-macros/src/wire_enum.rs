//! `#[wire_enum]` — the enum both decoded and sent: `Serialize`, `Deserialize`
//! and `JsonSchema` routed through `nest_rs_core`, and `async_graphql::Enum`
//! under its `graphql` key.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, Item, ItemEnum};

pub(crate) fn wire_enum(args: TokenStream, item: TokenStream) -> TokenStream {
    expand(args, item).unwrap_or_else(syn::Error::into_compile_error)
}

fn expand(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let graphql = parse_args(args)?;
    let item = parse_enum(item)?;

    if item.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "`#[wire_enum]` needs at least one variant — an empty enum has no wire representation",
        ));
    }
    for variant in &item.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                &variant.fields,
                "`#[wire_enum]` needs an enum whose variants are all unit variants — a GraphQL enum and a SeaORM `DeriveActiveEnum` column both require it; model a payload-carrying variant as a struct of its own",
            ));
        }
    }

    let graphql_derive = graphql.then(|| quote!(::nest_rs_graphql::async_graphql::Enum,));
    let graphql_crate =
        graphql.then(|| quote!(#[graphql(crate = "::nest_rs_graphql::async_graphql")]));

    // Our derive leads the developer's attributes: a helper (`#[serde(rename_all)]`)
    // above the derive that claims it trips `legacy_derive_helpers`.
    Ok(quote! {
        #[derive(
            ::core::clone::Clone,
            ::core::marker::Copy,
            ::core::fmt::Debug,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
            ::nest_rs_core::__private::serde::Serialize,
            ::nest_rs_core::__private::serde::Deserialize,
            #graphql_derive
            ::nest_rs_core::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_core::__private::serde")]
        #[schemars(crate = "::nest_rs_core::__private::schemars")]
        #graphql_crate
        #item
    })
}

/// `graphql` is explicit, not inferred from the crate feature: features unify
/// across a workspace, so one GraphQL app would derive `Enum` on every enum.
fn parse_args(args: TokenStream) -> syn::Result<bool> {
    const WIRE_ENUM: nest_rs_codegen::Grammar =
        nest_rs_codegen::Grammar::new("wire_enum", &["graphql"]);
    let mut graphql = false;
    WIRE_ENUM.parse2(args, |_| {
        graphql = true;
        Ok(())
    })?;
    Ok(graphql)
}

/// Parse the item, naming the struct's decorators when the developer decorated
/// one. Parsed as an [`Item`] first, so a syntax error inside an enum reports
/// that error, not "the other decorator".
fn parse_enum(item: TokenStream) -> syn::Result<ItemEnum> {
    match syn::parse2::<Item>(item)? {
        Item::Enum(item) => Ok(item),
        other => Err(syn::Error::new_spanned(
            other,
            "`#[wire_enum]` decorates an enum both decoded and sent. A struct takes `#[input]` \
             or `#[output]`, and an entity's `Model` `#[expose(name = \"…\")]`.",
        )),
    }
}
