use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, Expr};

const LAYER: nest_rs_codegen::Grammar = nest_rs_codegen::Grammar::new("layer", &["priority"]);

pub(crate) fn derive_layer(input: TokenStream) -> TokenStream {
    match syn::parse::<DeriveInput>(input).and_then(|item| layer(&item)) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `impl Layer` for `item`, its `priority` the one `#[layer(priority = …)]`
/// names, or the trait's default.
fn layer(item: &DeriveInput) -> syn::Result<TokenStream2> {
    let mut priority: Option<Expr> = None;
    let mut written = false;
    for attr in item
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("layer"))
    {
        if std::mem::replace(&mut written, true) {
            return Err(syn::Error::new_spanned(
                attr,
                "#[layer] is written once — put every key in one `#[layer(…)]`",
            ));
        }
        LAYER.parse_attr(attr, |arg| {
            priority = Some(arg.expr()?);
            Ok(())
        })?;
    }
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let priority = priority.map(|priority| {
        quote! {
            fn priority(&self) -> i8 {
                #priority
            }
        }
    });
    Ok(quote! {
        impl #impl_generics ::nest_rs_core::Layer for #name #ty_generics #where_clause {
            #priority
        }
    })
}
