use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, Expr};

const LAYER: nest_rs_codegen::Grammar = nest_rs_codegen::Grammar::new("layer", &["priority"]);

pub(crate) fn derive_layer(input: TokenStream) -> TokenStream {
    match syn::parse::<DeriveInput>(input).and_then(layer) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `impl Layer` for `item`, its `priority` the one `#[layer(priority = …)]`
/// names, or the trait's default.
fn layer(mut item: DeriveInput) -> syn::Result<TokenStream2> {
    let mut priority: Option<Expr> = None;
    if let Some(attr) = nest_rs_codegen::take_single_attr(&mut item.attrs, "layer")? {
        LAYER.parse_attr(&attr, |arg| {
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
