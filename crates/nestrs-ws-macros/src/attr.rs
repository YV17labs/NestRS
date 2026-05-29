//! Small `syn` attribute-parsing helpers shared by the WebSocket decorators.
//! A trimmed copy of the HTTP decorators' `attr` helpers — kept local so this
//! crate stays free of any dependency on `nestrs-http-macros`.

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

/// Extract and remove a `#[<ident>(PathA, PathB)]` attribute, returning its
/// comma-separated paths (empty when absent). Used for `#[use_guards]` on the
/// gateway struct. The attribute is *consumed* — removed from `attrs` so it
/// never reaches the compiler as an unknown attribute. At most one is accepted.
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
