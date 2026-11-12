//! `syn` attribute-parsing helpers local to the HTTP decorators.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::LitStr;

pub(crate) fn opt_str(value: &Option<LitStr>) -> TokenStream2 {
    match value {
        Some(lit) => quote! { ::core::option::Option::Some(#lit) },
        None => quote! { ::core::option::Option::None },
    }
}
