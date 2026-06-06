//! `#[dataloader]`: one batched DataLoader per method.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, ImplItem, Item, ItemImpl, PathArguments, ReturnType, Signature,
    Type, parse_macro_input,
};

use nest_rs_codegen::impl_self_ident;

pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(&args, "#[dataloader] takes no arguments")
            .to_compile_error()
            .into();
    }

    match parse_macro_input!(input as Item) {
        Item::Impl(item) => dataloader_impl(item),
        other => syn::Error::new_spanned(
            other,
            "#[dataloader] applies to a data-layer impl block; each method becomes a batched DataLoader",
        )
        .to_compile_error()
        .into(),
    }
}

fn dataloader_impl(item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();
    let base = match impl_self_ident(&self_ty, "#[dataloader]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };

    let mut loaders: Vec<TokenStream2> = Vec::new();
    for impl_item in &item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        match dataloader_for_method(&self_ty, &base, &method.sig) {
            Ok(loader) => loaders.push(loader),
            Err(err) => return err.to_compile_error().into(),
        }
    }

    quote! {
        #item

        #(#loaders)*
    }
    .into()
}

/// Generate one loader from `async fn name(&self, keys: &[K]) -> HashMap<K, V>`
/// (optionally wrapped in `Result<_, E>`).
fn dataloader_for_method(
    self_ty: &Type,
    base: &Ident,
    sig: &Signature,
) -> syn::Result<TokenStream2> {
    let key_ty = loader_key_type(sig)?;
    let (value_ty, error_ty) = loader_value_and_error(&sig.output)?;
    let method_name = &sig.ident;
    let loader_name = format_ident!("{}{}", base, pascal_case(method_name));
    let missing = format!(
        "{loader_name}: no provider registered for `{}`",
        quote!(#self_ty)
    );

    let call = if sig.asyncness.is_some() {
        quote! { self.0.#method_name(__keys).await }
    } else {
        quote! { self.0.#method_name(__keys) }
    };
    let (error_ty, load_body) = match error_ty {
        Some(err) => (quote!(#err), call),
        None => (
            quote!(::std::convert::Infallible),
            quote! { ::std::result::Result::Ok(#call) },
        ),
    };

    Ok(quote! {
        pub struct #loader_name(::std::sync::Arc<#self_ty>);

        impl #loader_name {
            fn from_container(container: &::nest_rs_core::Container) -> Self {
                Self(container.get::<#self_ty>().expect(#missing))
            }
        }

        impl ::nest_rs_graphql::async_graphql::dataloader::Loader<#key_ty> for #loader_name {
            type Value = #value_ty;
            type Error = #error_ty;

            async fn load(
                &self,
                __keys: &[#key_ty],
            ) -> ::std::result::Result<
                ::std::collections::HashMap<#key_ty, #value_ty>,
                Self::Error,
            > {
                #load_body
            }
        }

        ::nest_rs_graphql::inventory::submit! {
            ::nest_rs_graphql::LoaderRegistration {
                owner_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                // Built per request from the assembled container (so the
                // module's import order is irrelevant).
                seed: |__container, __request| {
                    let __loader = <#loader_name>::from_container(__container);
                    // Spawner re-installs the request's ambient executor +
                    // ability around each batch — a batch runs on a spawned
                    // task where task-locals are gone.
                    __request.data(
                        ::nest_rs_graphql::async_graphql::dataloader::DataLoader::new(
                            __loader,
                            ::nest_rs_graphql::batch_spawner(__container),
                        ),
                    )
                },
            }
        }
    })
}

fn pascal_case(ident: &Ident) -> Ident {
    let mut out = String::new();
    let mut upper = true;
    for ch in ident.to_string().chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    Ident::new(&out, ident.span())
}

fn loader_key_type(sig: &Signature) -> syn::Result<Type> {
    let mut inputs = sig.inputs.iter();
    if !matches!(inputs.next(), Some(FnArg::Receiver(_))) {
        return Err(syn::Error::new_spanned(
            sig,
            "#[dataloader] method needs a `&self` receiver",
        ));
    }
    let keys = inputs.next().ok_or_else(|| {
        syn::Error::new_spanned(sig, "#[dataloader] method needs a `keys: &[K]` argument")
    })?;
    let FnArg::Typed(pat) = keys else {
        return Err(syn::Error::new_spanned(
            keys,
            "#[dataloader] keys argument must be typed",
        ));
    };
    let Type::Reference(reference) = &*pat.ty else {
        return Err(syn::Error::new_spanned(
            &pat.ty,
            "#[dataloader] keys argument must be `&[K]`",
        ));
    };
    let Type::Slice(slice) = &*reference.elem else {
        return Err(syn::Error::new_spanned(
            &pat.ty,
            "#[dataloader] keys argument must be a slice `&[K]`",
        ));
    };
    Ok((*slice.elem).clone())
}

fn loader_value_and_error(output: &ReturnType) -> syn::Result<(Type, Option<Type>)> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "#[dataloader] method must return `HashMap<K, V>` or `Result<HashMap<K, V>, E>`",
        ));
    };
    match generic_args(ty, "Result") {
        Some(args) if args.len() == 2 => Ok((hashmap_value(&args[0])?, Some(args[1].clone()))),
        _ => Ok((hashmap_value(ty)?, None)),
    }
}

fn hashmap_value(ty: &Type) -> syn::Result<Type> {
    match generic_args(ty, "HashMap") {
        Some(args) if args.len() == 2 => Ok(args[1].clone()),
        _ => Err(syn::Error::new_spanned(
            ty,
            "#[dataloader] method must return a `HashMap<K, V>` (optionally in `Result<_, E>`)",
        )),
    }
}

fn generic_args(ty: &Type, expected: &str) -> Option<Vec<Type>> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != expected {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    Some(
        args.args
            .iter()
            .filter_map(|arg| match arg {
                GenericArgument::Type(t) => Some(t.clone()),
                _ => None,
            })
            .collect(),
    )
}
