//! Attribute-extraction helpers shared by the transport decorator macros —
//! finding, consuming and validating whole `#[...]` attributes off an item
//! (as opposed to [`crate::args`], which parses the values *inside* one).
//!
//! These gate the Layer-System surface (`#[use_guards]` / `#[force_guards]` /
//! `#[public]` and their HTTP-only siblings), so every transport reads them
//! from one place instead of keeping drifting copies.

use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Lit, LitStr, Path, Token};

/// Interpret an attribute-argument value as a string literal, cloning it out —
/// e.g. the `"…"` in `#[controller(path = "…")]` / `#[gateway(path = "…")]`.
/// Errors (spanned at the value) when it is not a string literal.
pub fn expr_str(expr: &Expr) -> syn::Result<LitStr> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}

/// Extract and remove a flag attribute (no args, no parens) like `#[public]`.
/// Returns `true` when present (and removes it), `false` when absent.
pub fn take_flag_attr(attrs: &mut Vec<Attribute>, ident: &str) -> bool {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return false;
    };
    attrs.remove(pos);
    true
}

/// Extract and remove a `#[<ident>(PathA, PathB)]` attribute, returning its
/// comma-separated paths (empty when absent). The attribute is consumed so it
/// never reaches the compiler as unknown. At most one is accepted; a second of
/// the same ident is rejected with a clear message — `noun` names the listed
/// element per call site (`"guard"`, `"entry"`).
pub fn take_path_list(
    attrs: &mut Vec<Attribute>,
    ident: &str,
    noun: &str,
) -> syn::Result<Vec<Path>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident(ident)) {
        return Err(syn::Error::new_spanned(
            &attr,
            format!("at most one `#[{ident}(...)]` is allowed; list every {noun} in it"),
        ));
    }
    Ok(attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

/// Reject `#[use_interceptors(...)]` / `#[use_filters(...)]` where they are
/// **HTTP-only** today: on transports with no per-message/per-operation seam
/// for those traits, binding one would be a silent no-op, so it is a named
/// compile error instead. Guards *are* bridged everywhere, so they stay.
/// `transport` (e.g. `"WebSockets"`, `"GraphQL"`) and `site` (e.g. `"gateway"`,
/// `"resolver"`) name the rejecting context in the diagnostic.
pub fn reject_http_only_layers(
    attrs: &[Attribute],
    transport: &str,
    site: &str,
) -> syn::Result<()> {
    for attr in attrs {
        for name in ["use_interceptors", "use_filters"] {
            if attr.path().is_ident(name) {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!(
                        "`#[{name}]` is not bridged on {transport} yet — it would be a silent \
                         no-op on a {site}. Remove it, or move the layer onto an HTTP \
                         `#[controller]` / `#[routes]`, where interceptors and filters run. \
                         Guards work on both.",
                    ),
                ));
            }
        }
    }
    Ok(())
}
