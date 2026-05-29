//! Attribute-argument parsing helpers shared by the decorator macros.

use proc_macro2::TokenStream as TokenStream2;
use syn::parse::{ParseStream, Parser};
use syn::{Ident, LitStr, Token};

/// Parse a decorator's sole `<key> = "..."` string argument from its attribute
/// tokens — `#[controller(path = "…")]`, `#[cron_job(every = "…")]`, etc. `key`
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
