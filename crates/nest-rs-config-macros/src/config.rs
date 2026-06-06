use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, ItemStruct, Lit, LitStr, MetaNameValue, Token, parse_macro_input};

pub(crate) fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    let namespace = match parse_namespace(args.into()) {
        Ok(ns) => ns,
        Err(err) => return err.to_compile_error().into(),
    };

    let item = parse_macro_input!(input as ItemStruct);
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let namespace_lit = namespace.value();

    quote! {
        #item

        impl #impl_generics ::nest_rs_config::Namespaced for #name #ty_generics #where_clause {
            const NAMESPACE: &'static str = #namespace_lit;
        }
    }
    .into()
}

fn parse_namespace(args: TokenStream2) -> syn::Result<LitStr> {
    let metas = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(args)?;

    let mut namespace: Option<LitStr> = None;
    for meta in metas {
        let key = meta
            .path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_default();
        match key.as_str() {
            "namespace" => namespace = Some(as_str_lit(&meta.value)?),
            other => {
                return Err(syn::Error::new_spanned(
                    &meta.path,
                    format!("unknown #[config] argument `{other}`; expected `namespace`"),
                ));
            }
        }
    }

    let lit = namespace.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[config] needs a namespace: `#[config(namespace = \"database\")]`",
        )
    })?;
    validate_namespace(&lit)?;
    Ok(lit)
}

fn as_str_lit(value: &Expr) -> syn::Result<LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = value
    {
        Ok(s.clone())
    } else {
        Err(syn::Error::new_spanned(
            value,
            "#[config] `namespace` must be a string literal, e.g. `namespace = \"database\"`",
        ))
    }
}

/// Lowercase env-domain segment so it round-trips into `NESTRS_<DOMAIN>__`.
fn validate_namespace(lit: &LitStr) -> syn::Result<()> {
    let value = lit.value();
    let valid = !value.is_empty()
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            lit.span(),
            "#[config] `namespace` must be a lowercase env-domain segment \
             (start with a letter, then lowercase letters, digits, or underscores), \
             e.g. \"database\" or \"object_store\"",
        ))
    }
}
