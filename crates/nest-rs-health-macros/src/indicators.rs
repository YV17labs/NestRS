//! `#[indicators]`: strips each probe attribute and submits one
//! `HealthIndicator` per method; `Discoverable` stays with `#[injectable]`.

use nest_rs_codegen::pair;
use nest_rs_codegen::{
    HostBorrow, await_if_async, cfg_attrs, impl_self_ident, returns_unit, shared_receiver,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ImplItem;
use syn::ext::IdentExt;

const PROBE_ATTRS: [(&str, &str); 3] = [
    ("liveness", "Liveness"),
    ("readiness", "Readiness"),
    ("startup", "Startup"),
];

pub(crate) fn indicators(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::INDICATORS
        .keep_item_on_refusal(
            written,
            expansion,
            &PROBE_ATTRS.map(|(name, _)| name),
            |_| TokenStream2::new(),
        )
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if let Err(err) = pair::INDICATORS.reject_args(&args, "the provider's scope is declared by") {
        return err.to_compile_error().into();
    }

    let mut item = match pair::INDICATORS.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let host_check = pair::INDICATORS.provider_host_check(&self_ty);
    let provider = match impl_self_ident(&self_ty, "#[indicators]") {
        Ok(ident) => ident,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_name = provider.unraw().to_string();

    let mut submissions: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // `take_flag_attr`: an argument such as `#[readiness(timeout = "5s")]`
        // is a compile error, never dropped.
        let accepted: Vec<&str> = PROBE_ATTRS.iter().map(|(name, _)| *name).collect();
        let index =
            match nest_rs_codegen::one_role_per_method("probe", &method.attrs, &accepted, "") {
                Ok(Some(index)) => index,
                Ok(None) => continue,
                Err(err) => return err.to_compile_error().into(),
            };
        let Some((name, kind_variant)) = PROBE_ATTRS
            .into_iter()
            .find(|(name, _)| method.attrs[index].path().is_ident(name))
        else {
            continue;
        };
        if let Err(err) = nest_rs_codegen::take_flag_attr(&mut method.attrs, name) {
            return err.to_compile_error().into();
        }

        if let Err(err) = shared_receiver(method, "#[indicators]", &provider, HostBorrow::Arc) {
            return err.to_compile_error().into();
        }
        if let Err(err) = nest_rs_codegen::concrete_signature(method, "#[indicators]") {
            return err.to_compile_error().into();
        }

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.unraw().to_string();
        let kind_ident = syn::Ident::new(kind_variant, method_ident.span());
        let cfgs = cfg_attrs(&method.attrs);

        let call = await_if_async(&method.sig, quote!(<#self_ty>::#method_ident(&__provider)));
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
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_health::HealthIndicator {
                    origin: ::core::module_path!(),
                    name: #method_name,
                    kind: ::nest_rs_health::ProbeKind::#kind_ident,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    run: |__container| ::std::boxed::Box::pin(async move {
                        // Not an `expect`: a panic would take the probe response down.
                        let __provider = match ::nest_rs_core::Container::get::<#self_ty>(
                            __container,
                        ) {
                            ::std::option::Option::Some(__host) => __host,
                            ::std::option::Option::None => {
                                return ::std::result::Result::Err(
                                    ::nest_rs_health::__private::unresolved_host(#provider_name),
                                );
                            }
                        };
                        #invoke
                    }),
                }
            }
        });
    }

    let out = quote! {
        #item

        #host_check
        #(#submissions)*
    };
    out.into()
}
