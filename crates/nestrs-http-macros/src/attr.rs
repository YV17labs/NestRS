//! Small `syn` attribute-parsing helpers shared by the HTTP decorators.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Expr, Lit, LitStr};

/// A `key = "..."` value must be a string literal.
pub(crate) fn expr_str(expr: &Expr) -> syn::Result<LitStr> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}

/// `Some(lit)` → `Some("lit")` tokens, `None` → `None` tokens.
pub(crate) fn opt_str(value: &Option<LitStr>) -> TokenStream2 {
    match value {
        Some(lit) => quote! { ::core::option::Option::Some(#lit) },
        None => quote! { ::core::option::Option::None },
    }
}
