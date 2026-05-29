//! `#[expose]` implementation: parse the entity, then emit the GraphQL output
//! object, the `Create`/`Update` inputs, and the `ActiveModel` conversions, and
//! re-emit the entity untouched.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

use crate::{active, attr, dto, input};

pub(crate) fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemStruct);
    let model = match attr::parse(args.into(), &mut item) {
        Ok(model) => model,
        Err(err) => return err.to_compile_error().into(),
    };

    let output = dto::emit(&model);
    let inputs = input::emit(&model);
    let active = active::emit(&model);

    quote! {
        #item
        #output
        #inputs
        #active
    }
    .into()
}
