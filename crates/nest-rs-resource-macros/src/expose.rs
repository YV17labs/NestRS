use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

use crate::{active, attr, dto, input, lifecycle, relations, wire};

pub(crate) fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemStruct);
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
