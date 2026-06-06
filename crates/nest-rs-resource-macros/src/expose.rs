use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

use crate::{active, attr, dto, input, wire};

pub(crate) fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemStruct);
    let model = match attr::parse(args.into(), &mut item) {
        Ok(model) => model,
        Err(err) => return err.to_compile_error().into(),
    };

    let output = dto::emit(&model);
    let inputs = input::emit(&model);
    let active = active::emit(&model);
    let wire_defaults = wire::emit(&model);

    quote! {
        #item
        #output
        #inputs
        #active
        #wire_defaults
    }
    .into()
}
