//! Small `syn` attribute-parsing helpers shared by the HTTP decorators.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Lit, LitStr, Path, Token};

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

/// Extract and remove a `#[<ident>(PathA, PathB)]` attribute, returning its comma-
/// separated paths (empty when absent). Used for the `#[use_guards]` /
/// `#[use_filters]` / `#[use_interceptors]` decorators, on both a controller struct
/// and a handler method. The attribute is *consumed* — removed from `attrs` so it
/// never reaches the compiler as an unknown attribute. At most one is accepted;
/// a second of the same ident is rejected with a clear message rather than left to
/// surface as a confusing "cannot find attribute" error.
pub(crate) fn take_use_attr(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<Vec<Path>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident(ident)) {
        return Err(syn::Error::new_spanned(
            &attr,
            format!("at most one `#[{ident}(...)]` is allowed; list every entry in it"),
        ));
    }
    Ok(attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}
