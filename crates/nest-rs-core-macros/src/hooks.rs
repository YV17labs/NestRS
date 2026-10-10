use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::ImplItem;
use syn::ext::IdentExt;

use nest_rs_codegen::{
    HostBorrow, await_if_async, cfg_attrs, impl_self_ident, returns_unit, shared_receiver,
};

const HOOK_ATTRS: [(&str, &str); 5] = [
    ("on_module_init", "OnModuleInit"),
    ("on_application_bootstrap", "OnApplicationBootstrap"),
    ("on_module_destroy", "OnModuleDestroy"),
    ("before_application_shutdown", "BeforeApplicationShutdown"),
    ("on_application_shutdown", "OnApplicationShutdown"),
];

pub(crate) fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::HOOKS
        .keep_item_on_refusal(
            written,
            expansion,
            &HOOK_ATTRS.map(|(name, _)| name),
            |_| TokenStream2::new(),
        )
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if let Err(err) = pair::HOOKS.reject_args(&args, "the provider's scope is declared by") {
        return err.to_compile_error().into();
    }

    let mut item = match pair::HOOKS.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let base = match impl_self_ident(&self_ty, "#[hooks]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_lit = base.unraw().to_string();
    let host_check = pair::HOOKS.provider_host_check(&self_ty);

    let mut submissions: Vec<TokenStream2> = Vec::new();
    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let accepted: Vec<&str> = HOOK_ATTRS.iter().map(|(name, _)| *name).collect();
        let index = match nest_rs_codegen::one_role_per_method(
            "lifecycle phase",
            &method.attrs,
            &accepted,
            "",
        ) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };
        let Some((name, phase)) = HOOK_ATTRS
            .into_iter()
            .find(|(name, _)| method.attrs[index].path().is_ident(name))
        else {
            continue;
        };
        if let Err(err) = nest_rs_codegen::take_flag_attr(&mut method.attrs, name) {
            return err.to_compile_error().into();
        }
        let phase_variant = format_ident!("{}", phase);

        if let Err(err) = shared_receiver(method, "#[hooks]", &base, HostBorrow::Arc) {
            return err.to_compile_error().into();
        }
        if let Err(err) = nest_rs_codegen::concrete_signature(method, "#[hooks]") {
            return err.to_compile_error().into();
        }

        let method_name = method.sig.ident.clone();
        let method_lit = method_name.unraw().to_string();
        let run_fn = format_ident!("__nestrs_hook_{}_{}", base, method_name);
        let cfgs = cfg_attrs(&method.attrs);

        // Called by its path, never as `__provider.method()`: method lookup tries
        // the `Arc` first, so a trait method of that name on `Arc<T>` would win.
        let call = await_if_async(&method.sig, quote!(<#self_ty>::#method_name(&__provider)));
        let invoke = if returns_unit(&method.sig.output) {
            quote! {
                #call;
                ::std::result::Result::Ok(())
            }
        } else {
            quote! {
                ::std::result::Result::map_err(#call, ::std::convert::Into::into)
            }
        };

        submissions.push(quote! {
            #(#cfgs)*
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #run_fn(
                __container: &::nest_rs_core::Container,
            ) -> ::std::pin::Pin<::std::boxed::Box<
                dyn ::std::future::Future<Output = ::nest_rs_core::anyhow::Result<()>>
                    + ::std::marker::Send
                    + '_,
            >> {
                ::std::boxed::Box::pin(async move {
                    match ::nest_rs_core::Container::get::<#self_ty>(__container) {
                        ::std::option::Option::Some(__provider) => { #invoke }
                        ::std::option::Option::None => ::std::result::Result::Ok(()),
                    }
                })
            }

            #(#cfgs)*
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_core::__private::LifecycleHook {
                    phase: ::nest_rs_core::LifecyclePhase::#phase_variant,
                    provider: #provider_lit,
                    method: #method_lit,
                    origin: ::core::module_path!(),
                    provider_type_id: ::std::any::TypeId::of::<#self_ty>,
                    present: |__container| ::std::option::Option::is_some(
                        &::nest_rs_core::Container::get::<#self_ty>(__container),
                    ),
                    run: #run_fn,
                }
            }
        });
    }

    quote! {
        #item

        #host_check

        #(#submissions)*
    }
    .into()
}
