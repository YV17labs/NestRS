//! Attribute-argument parsing helpers shared by the decorator macros.

use proc_macro2::TokenStream as TokenStream2;
use syn::parse::{ParseStream, Parser};
use syn::{Expr, ExprLit, Ident, Lit, LitStr, Token};

/// Parse a decorator's sole `<key> = "..."` string argument from its attribute
/// tokens — `#[controller(path = "…")]`, `#[mcp(path = "…")]`, etc. `key`
/// is the expected argument name, `attr` the attribute; both appear in the error.
pub fn parse_named_str_arg(args: TokenStream2, key: &str, attr: &str) -> syn::Result<LitStr> {
    let parser = |input: ParseStream| -> syn::Result<LitStr> {
        let found: Ident = input.parse()?;
        if found != key {
            return Err(syn::Error::new(
                found.span(),
                format!("expected `{key} = \"...\"` as the only #[{attr}] argument"),
            ));
        }
        input.parse::<Token![=]>()?;
        input.parse()
    };
    parser.parse2(args)
}

/// Interpret an already-parsed attribute-argument value as a string literal,
/// cloning it out — the value half of a `syn::MetaNameValue` you already hold,
/// as opposed to [`parse_named_str_arg`], which parses the whole `key = "..."`.
/// On a non-string value it errors (spanned at the value) with a message naming
/// the decorator and key — ``#[{attr}] `{key}` must be a string literal, e.g.
/// `{key} = "{example}"` `` — where `example` is the placeholder value shown in
/// the hint (`"database"`, `"..."`).
pub fn require_str_lit(value: &Expr, attr: &str, key: &str, example: &str) -> syn::Result<LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = value
    {
        Ok(s.clone())
    } else {
        Err(syn::Error::new_spanned(
            value,
            format!("#[{attr}] `{key}` must be a string literal, e.g. `{key} = \"{example}\"`"),
        ))
    }
}
