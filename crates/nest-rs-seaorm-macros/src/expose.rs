use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Item, ItemStruct};

use crate::{active, attr, dto, input, lifecycle, refused_wires, relations};

pub(crate) fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = match parse_struct(item.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut model = match attr::parse(args.into(), &mut item) {
        Ok(model) => model,
        Err(err) => return err.to_compile_error().into(),
    };

    if model.graphql {
        #[cfg(not(feature = "graphql"))]
        {
            return nest_rs_codegen::__private::refused_without(
                "`#[expose(..., graphql)]`",
                "nest_rs_graphql",
            )
            .unwrap_or_else(|| {
                syn::Error::new_spanned(
                    &model.source_ident,
                    "`#[expose(..., graphql)]` requires the `graphql` feature on `nest-rs-seaorm`",
                )
                .to_compile_error()
            })
            .into();
        }
    } else if model.has_auto_relations() {
        return syn::Error::new_spanned(
            &model.source_ident,
            "an exposed SeaORM relation requires `#[expose(..., graphql)]` on the entity — use scalar FK columns for HTTP-only entities, or leave the relation unexposed (no `#[expose]`)",
        )
        .to_compile_error()
        .into();
    }

    if model.graphql && !model.complex && model.has_auto_relations() {
        model.complex = true;
    }

    if model.complex && !model.graphql {
        return syn::Error::new_spanned(
            &model.source_ident,
            "`#[expose(complex)]` requires `graphql` — the wire DTO has no GraphQL object shape",
        )
        .to_compile_error()
        .into();
    }

    let derive_refused = refused_wires::derive_refusals(&item.attrs, &model);
    // A derive just refused would overlap these too, one misuse said twice.
    let wires_refused = derive_refused
        .is_none()
        .then(|| refused_wires::emit(&item.attrs, &model));
    let output = dto::emit(&model);
    let inputs = input::emit(&model);
    let active = active::emit(&model);
    let lifecycle = lifecycle::emit(&model);
    let relations = if model.graphql {
        match relations::emit(&model) {
            Ok(tokens) => tokens,
            Err(err) => return err.to_compile_error().into(),
        }
    } else {
        quote! {}
    };

    quote! {
        #derive_refused
        #item
        #output
        #inputs
        #active
        #wires_refused
        #lifecycle
        #relations
    }
    .into()
}

/// Parse the entity struct, naming `#[wire_enum]` when the developer decorated a
/// column's enum type instead. Parsed as an [`Item`] first, so a syntax error
/// inside a struct reports that error, not "the other decorator".
fn parse_struct(item: TokenStream2) -> syn::Result<ItemStruct> {
    match syn::parse2::<Item>(item)? {
        Item::Struct(item) => Ok(item),
        other => Err(syn::Error::new_spanned(
            other,
            "`#[expose(name = \"…\")]` decorates the entity `Model` struct — the row. A column's \
             enum type takes `#[wire_enum]` instead, which is what carries the wire derives for \
             an enum.",
        )),
    }
}
