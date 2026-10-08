//! `#[main]` — the binary's entry point. Not `main.rs`, which Cargo would build
//! as a binary target of this crate.

use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned as _;
use syn::{Block, ItemFn, ReturnType};

/// Each refusal is emitted beside the item it refused, or rustc adds a second
/// error: `main` function not found.
pub(crate) fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    let refused = nest_rs_codegen::entry_takes_no_arguments(&args.into())
        .map(|refused| refused.to_compile_error());
    let input = proc_macro2::TokenStream::from(input);
    let mut item: ItemFn = match syn::parse2(input.clone()) {
        Ok(item) => item,
        Err(_) => {
            let error = nest_rs_codegen::entry_needs_an_async_fn(input.span()).to_compile_error();
            return quote! { #error #input }.into();
        }
    };
    if item.sig.asyncness.take().is_none() {
        let error =
            nest_rs_codegen::entry_needs_an_async_fn(item.sig.fn_token.span).to_compile_error();
        return quote! { #error #input }.into();
    }
    // Named so the body's `?`s convert into it: an `async` block infers its
    // output from its tail alone, and `Ok(())` names no error type.
    let output = match &item.sig.output {
        ReturnType::Default => quote! { () },
        ReturnType::Type(_, ty) => quote! { #ty },
    };
    let body = &item.block;
    let wrapped: Block = syn::parse_quote! {{
        ::nest_rs_core::__main::<#output, _>(async move #body)
    }};
    *item.block = wrapped;
    quote! { #refused #item }.into()
}
