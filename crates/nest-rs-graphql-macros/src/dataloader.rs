//! `#[dataloader]`: one batched DataLoader per method.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, ImplItem, Item, ItemImpl, PathArguments, ReturnType, Signature,
    Type, parse_macro_input,
};

use nest_rs_codegen::{cfg_attrs, impl_self_ident, pascal_case};

pub(crate) fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
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
        match dataloader_for_method(&self_ty, &base, &method.sig, &cfg_attrs(&method.attrs)) {
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
    cfgs: &[TokenStream2],
) -> syn::Result<TokenStream2> {
    let key_ty = loader_key_type(sig)?;
    let (value_ty, error_ty) = loader_value_and_error(&sig.output)?;
    let method_name = &sig.ident;
    let loader_name = format_ident!("{}{}", base, pascal_case(method_name));
    let missing = format!(
        "{loader_name}: no provider registered for `{}`",
        quote!(#self_ty)
    );
    let loader_doc = format!(
        "Auto-generated DataLoader for `{base}::{method_name}`, emitted by \
         `#[dataloader]`. Its name follows the framework convention \
         `{{Owner}}{{PascalMethod}}` — owner struct + PascalCase method — so \
         this loader is `{loader_name}`. Reference it by exactly that name from \
         a relation or a `#[field_resolver]`. Each batch runs `{base}::{method_name}` \
         through `Repo` (ability-scoped, per request)."
    );

    // By path, never `self.0.method(..)`: method lookup tries the `Arc` first, so
    // a same-named trait method on `Arc<T>` would run instead.
    let call =
        nest_rs_codegen::await_if_async(sig, quote! { <#self_ty>::#method_name(&*self.0, __keys) });
    let (error_ty, load_body) = match error_ty {
        Some(err) => (quote!(#err), call),
        None => (
            quote!(::std::convert::Infallible),
            quote! { ::std::result::Result::Ok(#call) },
        ),
    };

    Ok(quote! {
        #(#cfgs)*
        #[doc = #loader_doc]
        pub struct #loader_name(::std::sync::Arc<#self_ty>);

        #(#cfgs)*
        impl #loader_name {
            fn from_container(container: &::nest_rs_core::Container) -> Self {
                Self(container.get::<#self_ty>().expect(#missing))
            }
        }

        #(#cfgs)*
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

        #(#cfgs)*
        ::nest_rs_graphql::__private::inventory::submit! {
            ::nest_rs_graphql::__private::GraphqlLoaderRegistration {
                owner_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                seed: |__container, __batches, __request| {
                    let __loader = <#loader_name>::from_container(__container);
                    // A batch runs on a spawned task where task-locals are gone:
                    // the spawner re-installs the executor and ability.
                    __request.data(
                        ::nest_rs_graphql::async_graphql::dataloader::DataLoader::new(
                            __loader,
                            ::nest_rs_graphql::__private::batch_spawner(__container, __batches),
                        ),
                    )
                },
            }
        }
    })
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
