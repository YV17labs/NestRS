//! `#[wire_enum]` — the enum mode of `#[expose]`.
//!
//! An `#[expose]`d column of a custom enum type passes through `dto.rs`
//! verbatim, so the enum itself has to carry the wire traits. Hand-written that
//! is `Serialize`, `Deserialize`, `JsonSchema` and — under `graphql` —
//! `async_graphql::Enum`, none of which can be reached without naming its crate:
//! a derive expands against the *call site's* prelude, so the entity crate ended
//! up declaring `schemars` and `async-graphql` for code it never wrote. That is
//! the defect *The umbrella is the front door* names.
//!
//! This decorator emits those derives with their `crate = ` overrides routed
//! through `nest-rs-resource`, plus the `Clone`/`Copy`/`Eq` shape both
//! `async_graphql::Enum` and SeaORM's `DeriveActiveEnum` require of a unit-only
//! enum. It deliberately emits **no** SeaORM half: `EnumIter`,
//! `DeriveActiveEnum`, `#[sea_orm(rs_type, db_type)]` and the per-variant
//! `string_value` are entity-site code the developer's own source legitimately
//! writes, and the column's storage type is not ours to choose.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Fields, Item, ItemEnum};

use crate::attr::{graphql_root, graphql_root_str};

/// This decorator as written, for the sentence [`crate::expose`] prints when it
/// is handed a column's enum. Each half names the *other*, from the other's own
/// constant, so neither message can come to name a decorator that moved.
pub(crate) const NAME: &str = "#[wire_enum]";

/// The sibling decorator a `#[wire_enum]` on the wrong item shape names, so the
/// diagnostic points at the decorator that *does* take a struct rather than at
/// syn's `expected enum`.
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

    // Emitted **before** the developer's own attributes: `#[serde(rename_all =
    // …)]` and `#[graphql(name = …)]` are derive helper attributes, and one
    // written above the derive that claims it trips `legacy_derive_helpers`
    // (warn-by-default, future-incompatible). Our derive therefore leads, and
    // whatever the developer wrote — including their own
    // `#[derive(EnumIter, DeriveActiveEnum)]` and the `#[sea_orm(…)]` it claims
    // — follows in the order they wrote it.
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

/// `graphql` is the only option, and it means what it means on `#[expose]`:
/// also emit the GraphQL surface. Kept explicit rather than inferred from the
/// crate feature, because a Cargo feature is additive across a workspace — one
/// GraphQL app would otherwise silently put an `Enum` derive on every enum in
/// every sibling crate.
fn parse_args(args: TokenStream2) -> syn::Result<bool> {
    let mut graphql = false;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("graphql") {
            nest_rs_codegen::once(graphql, &meta.path, "wire_enum", "graphql")?;
            graphql = true;
            Ok(())
        } else {
            let name = nest_rs_codegen::key_as_written(&meta.path);
            Err(meta.error(nest_rs_codegen::unknown_argument(
                "wire_enum",
                &name,
                &["graphql"],
            )))
        }
    });
    syn::parse::Parser::parse2(parser, args)?;
    Ok(graphql)
}

/// Parse the item, naming [`HOST`] when the developer decorated the entity
/// struct instead. The item is parsed as an [`Item`] *before* the shape is
/// judged, so a genuine syntax error inside an enum reports that error rather
/// than "you wanted the other decorator".
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

/// `async_graphql::Enum` spliced into the derive list, routed through the
/// surface crate. Present only under `#[wire_enum(graphql)]` — an enum reaching
/// HTTP alone needs no GraphQL impls, and `#[graphql(crate = …)]` would be an
/// unclaimed attribute without the derive that owns it.
fn graphql_enum_derive(graphql: bool) -> TokenStream2 {
    if !graphql {
        return TokenStream2::new();
    }
    let root = graphql_root();
    quote! { #root::Enum, }
}

/// The `crate = ` override that derive needs — see `attr::graphql_crate_attr`
/// for why a bare `::async_graphql` would put the crate in the *consumer's*
/// manifest.
fn graphql_crate_attr(graphql: bool) -> TokenStream2 {
    if !graphql {
        return TokenStream2::new();
    }
    let root = graphql_root_str();
    quote! { #[graphql(crate = #root)] }
}
