use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Item, ItemStruct};

use crate::{active, attr, dto, input, lifecycle, relations, wire};

/// This decorator as written, for the sentence [`crate::wire_enum`] prints when
/// it is handed the entity struct. Each half names the *other*, from the other's
/// own constant, so neither message can come to name a decorator that moved.
pub(crate) const NAME: &str = "#[expose(name = \"…\")]";

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
            return syn::Error::new_spanned(
                &model.source_ident,
                "`#[expose(..., graphql)]` requires the `graphql` feature on `nest-rs-resource` (`features = [\"graphql\"]`)",
            )
            .to_compile_error()
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

    let output = dto::emit(&model);
    let inputs = input::emit(&model);
    let active = active::emit(&model);
    let wire_defaults = wire::emit(&model);
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
        #item
        #output
        #inputs
        #active
        #wire_defaults
        #lifecycle
        #relations
    }
    .into()
}

/// Parse the entity struct, naming [`crate::wire_enum`] when the developer
/// decorated a column's enum type instead — the mistake this pair exists to
/// absorb, since "make this reach the wire" is one intent with two item shapes.
/// The item is parsed as an [`Item`] *before* the shape is judged, so a genuine
/// syntax error inside a struct reports that error rather than "you wanted the
/// other decorator".
fn parse_struct(item: TokenStream2) -> syn::Result<ItemStruct> {
    match syn::parse2::<Item>(item)? {
        Item::Struct(item) => Ok(item),
        other => Err(syn::Error::new_spanned(
            other,
            format!(
                "`{NAME}` decorates the entity `Model` struct — the row. A column's enum \
                 type takes `{enum_half}` instead, which is what carries the wire derives \
                 for an enum.",
                enum_half = crate::wire_enum::NAME,
            ),
        )),
    }
}
