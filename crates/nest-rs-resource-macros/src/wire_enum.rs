//! `#[wire_enum]` — the enum mode of `#[expose]`.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Fields, Item, ItemEnum};

use crate::attr::{graphql_root, graphql_root_str};

/// This decorator as written, for the sentence [`#[expose]`](macro@crate::expose) prints when it
/// is handed a column's enum.
pub(crate) const NAME: &str = "#[wire_enum]";

/// The sibling decorator a `#[wire_enum]` on the wrong item shape names.
const HOST: &str = crate::expose::NAME;

pub(crate) fn wire_enum(args: TokenStream2, item: TokenStream2) -> TokenStream2 {
    match expand(args, item) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error(),
    }
}

fn expand(args: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
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
                "`#[wire_enum]` needs an enum whose variants are all unit variants — a GraphQL enum and a SeaORM `DeriveActiveEnum` column both require it; model a payload-carrying variant as its own `#[expose]`d entity",
            ));
        }
    }

    #[cfg(not(feature = "graphql"))]
    if graphql {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "`#[wire_enum(graphql)]` requires the `graphql` feature on `nest-rs-resource` (`features = [\"graphql\"]`)",
        ));
    }

    // Our derive leads the developer's attributes: a helper (`#[serde(rename_all)]`)
    // above the derive that claims it trips `legacy_derive_helpers`.
    let graphql_derive = graphql_enum_derive(graphql);
    let graphql_crate = graphql_crate_attr(graphql);

    Ok(quote! {
        #[derive(
            ::core::clone::Clone,
            ::core::marker::Copy,
            ::core::fmt::Debug,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
            ::nest_rs_resource::serde::Serialize,
            ::nest_rs_resource::serde::Deserialize,
            #graphql_derive
            ::nest_rs_resource::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_resource::serde")]
        #[schemars(crate = "::nest_rs_resource::schemars")]
        #graphql_crate
        #item
    })
}

/// `graphql` is explicit, not inferred from the crate feature: features unify
/// across a workspace, so one GraphQL app would derive `Enum` on every enum.
fn parse_args(args: TokenStream2) -> syn::Result<bool> {
    const WIRE_ENUM: nest_rs_codegen::Grammar =
        nest_rs_codegen::Grammar::new("wire_enum", &["graphql"]);
    let mut graphql = false;
    WIRE_ENUM.parse2(args, |_| {
        graphql = true;
        Ok(())
    })?;
    Ok(graphql)
}

/// Parse the item, naming [`HOST`] when the developer decorated the entity
/// struct instead. Parsed as an [`Item`] first, so a syntax error inside an
/// enum reports that error, not "the other decorator".
fn parse_enum(item: TokenStream2) -> syn::Result<ItemEnum> {
    match syn::parse2::<Item>(item)? {
        Item::Enum(item) => Ok(item),
        other => Err(syn::Error::new_spanned(
            other,
            format!(
                "`#[wire_enum]` decorates an enum — a column's type. The entity `Model` struct \
                 that carries the column takes `{HOST}` instead.",
            ),
        )),
    }
}

/// `async_graphql::Enum` spliced into the derive list, under
/// `#[wire_enum(graphql)]` only.
fn graphql_enum_derive(graphql: bool) -> TokenStream2 {
    if !graphql {
        return TokenStream2::new();
    }
    let root = graphql_root();
    quote! { #root::Enum, }
}

/// The `crate = ` override that derive needs.
fn graphql_crate_attr(graphql: bool) -> TokenStream2 {
    if !graphql {
        return TokenStream2::new();
    }
    let root = graphql_root_str();
    quote! { #[graphql(crate = #root)] }
}
