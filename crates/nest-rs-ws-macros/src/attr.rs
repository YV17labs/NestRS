//! Local copy of the HTTP decorators' attribute helpers, so this crate stays
//! free of any dep on `nest-rs-http-macros`.

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

/// `#[use_interceptors(...)]` / `#[use_filters(...)]` are **HTTP-only** today:
/// the per-message WS seam (`wrap_ws`) is reserved but not invoked, so binding
/// an interceptor or filter on a gateway or message handler would be a silent
/// no-op. Reject it at compile time with a named error. Guards *are* bridged
/// (the upgrade reuses the HTTP guard chain), so they stay.
pub(crate) fn reject_http_only_layers(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs {
        for name in ["use_interceptors", "use_filters"] {
            if attr.path().is_ident(name) {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!(
                        "`#[{name}]` is not bridged on WebSockets yet — it would be a silent no-op \
                         on a gateway. Remove it, or move the layer onto an HTTP `#[controller]` / \
                         `#[routes]`, where interceptors and filters run. Guards work on both.",
                    ),
                ));
            }
        }
    }
    Ok(())
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
