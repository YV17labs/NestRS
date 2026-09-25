//! `#[indicators]` — orchestrator on a provider's `impl` block. Walks the
//! methods, finds those tagged with `#[liveness]` / `#[readiness]` /
//! `#[startup]`, strips the attribute, and submits one `HealthIndicator` per
//! method to the link-time inventory. The methods stay on the impl block
//! unchanged so they remain regular methods callable from anywhere.
//!
//! Discoverable is NOT emitted here — the provider's own `#[injectable]` owns
//! it. Inventory is exactly the seam `#[hooks]`, `#[scheduled]`, and
//! `#[processor]` use, for the same reason.

use nest_rs_codegen::{
    DecoratorPair, HostBorrow, await_if_async, cfg_attrs, impl_self_ident, returns_unit,
    shared_receiver,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ImplItem;
use syn::ext::IdentExt;

/// The indicator host keeps its own `#[injectable]`; this names the shape
/// `#[indicators]` wants rather than reporting syn's `expected impl`.
const INDICATORS_PAIR: DecoratorPair =
    DecoratorPair::on_provider("#[indicators]", "#[liveness] / #[readiness] / #[startup]");

const PROBE_ATTRS: [(&str, &str); 3] = [
    ("liveness", "Liveness"),
    ("readiness", "Readiness"),
    ("startup", "Startup"),
];

pub(crate) fn indicators(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    INDICATORS_PAIR
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
    if let Err(err) = INDICATORS_PAIR.reject_args(&args, "the provider's scope is declared by") {
        return err.to_compile_error().into();
    }

    let mut item = match INDICATORS_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let host_check = INDICATORS_PAIR.provider_host_check(&self_ty);
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

        // One probe per method, through the family's helper, and that one taken
        // through `take_flag_attr`, so an argument on it —
        // `#[readiness(timeout = "5s")]` compiled and meant nothing — is a named
        // compile error rather than something dropped.
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

        // Adapt the method's return to `anyhow::Result<()>`. A method answering
        // `()` — written or not — is infallible (always `up`); any other must
        // yield `Result<(), E: Into<anyhow::Error>>`.
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
                        // Not an `expect`: this runs inside a probe request, so a
                        // panic would take the response down where an `Err` is
                        // reported as `down` with the reason on the crate's `warn`.
                        // The sentence is the kernel's — see `unresolved_host`.
                        let __provider = match ::nest_rs_core::Container::get::<#self_ty>(
                            __container,
                        ) {
                            ::std::option::Option::Some(__host) => __host,
                            ::std::option::Option::None => {
                                return ::std::result::Result::Err(
                                    ::nest_rs_health::unresolved_host(#provider_name),
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
