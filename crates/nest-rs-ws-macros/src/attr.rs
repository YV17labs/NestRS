//! Local copy of the HTTP decorators' attribute helpers, so this crate stays
//! free of any dep on `nestrs-http-macros`.

use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Lit, LitStr, Path, Token};

pub(crate) fn expr_str(expr: &Expr) -> syn::Result<LitStr> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}

/// Consumes the attribute so it never reaches the compiler as unknown.
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

/// Consumes a marker attribute (`#[public]`, etc.) so it never reaches the
/// compiler as unknown. Returns `true` when present.
pub(crate) fn take_flag_attr(attrs: &mut Vec<Attribute>, ident: &str) -> bool {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return false;
    };
    attrs.remove(pos);
    true
}
