use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

use crate::{active, attr, dto, input, relations, wire};

pub(crate) fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemStruct);
    let mut model = match attr::parse(args.into(), &mut item) {
        Ok(model) => model,
        Err(err) => return err.to_compile_error().into(),
    };

    // An auto-resolved relation implies `#[graphql(complex)]` on the wire
    // output — the macro attaches a `#[ComplexObject]` for the field
    // resolvers. Setting it here lets users omit the explicit `complex`.
    if model.has_auto_relations() {
        model.complex = true;
    }

    let output = dto::emit(&model);
    let inputs = input::emit(&model);
    let active = active::emit(&model);
    let wire_defaults = wire::emit(&model);
    let relations = match relations::emit(&model) {
        Ok(tokens) => tokens,
        Err(err) => return err.to_compile_error().into(),
    };

    quote! {
        #item
        #output
        #inputs
        #active
        #wire_defaults
        #relations
    }
    .into()
}
