use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{ImplItem, ReturnType};

use nest_rs_codegen::{DecoratorPair, impl_self_ident};

/// A lifecycle host keeps its own `#[injectable]`; this names the shape
/// `#[hooks]` wants rather than reporting syn's `expected impl`.
const HOOKS_PAIR: DecoratorPair = DecoratorPair::on_provider(
    "#[hooks]",
    "#[on_module_init] / #[on_application_bootstrap] / #[on_module_destroy]",
);

const HOOK_ATTRS: [(&str, &str); 5] = [
    ("on_module_init", "OnModuleInit"),
    ("on_application_bootstrap", "OnApplicationBootstrap"),
    ("on_module_destroy", "OnModuleDestroy"),
    ("before_application_shutdown", "BeforeApplicationShutdown"),
    ("on_application_shutdown", "OnApplicationShutdown"),
];

pub fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if let Err(err) = HOOKS_PAIR.reject_args(&args, "the provider's scope is declared by") {
        return err.to_compile_error().into();
    }

    let mut item = match HOOKS_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let base = match impl_self_ident(&self_ty, "#[hooks]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_lit = base.to_string();
    let host_check = HOOKS_PAIR.provider_host_check(&self_ty);

    let mut submissions: Vec<TokenStream2> = Vec::new();
    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let phase = method.attrs.iter().enumerate().find_map(|(idx, attr)| {
            HOOK_ATTRS
                .iter()
                .find(|(name, _)| attr.path().is_ident(name))
                .map(|(_, variant)| (idx, *variant))
        });
        let Some((idx, phase)) = phase else { continue };
        method.attrs.remove(idx);
        let phase_variant = format_ident!("{}", phase);

        if method.sig.asyncness.is_none() {
            return syn::Error::new_spanned(&method.sig, "#[hooks] methods must be `async fn`")
                .to_compile_error()
                .into();
        }

        let method_name = method.sig.ident.clone();
        let method_lit = method_name.to_string();
        let run_fn = format_ident!("__nestrs_hook_{}_{}", base, method_name);

        // Adapt the method's return to `anyhow::Result<()>`: a bare method is
        // infallible, a returning one must yield `Result<(), E: Into<_>>`.
        let invoke = match &method.sig.output {
            ReturnType::Default => quote! {
                __provider.#method_name().await;
                ::std::result::Result::Ok(())
            },
            ReturnType::Type(..) => quote! {
                ::std::result::Result::map_err(
                    __provider.#method_name().await,
                    ::std::convert::Into::into,
                )
            },
        };

        submissions.push(quote! {
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

            ::nest_rs_core::inventory::submit! {
                ::nest_rs_core::LifecycleHook {
                    phase: ::nest_rs_core::LifecyclePhase::#phase_variant,
                    provider: #provider_lit,
                    method: #method_lit,
                    origin: ::core::module_path!(),
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
