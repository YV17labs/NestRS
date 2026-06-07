//! `#[listeners]` — orchestrator on a provider's `impl` block. Walks the
//! methods; for each one tagged with `#[on_event]`, emits a free `wire` fn
//! that resolves the provider from the assembled container and subscribes a
//! closure to the [`EventBus`], then submits a `ListenerMethod` inventory
//! entry the [`EventModule`] drains at bootstrap.
//!
//! Mirrors `#[processor]`/`#[process]` and `#[scheduled]`/`#[every]`: the
//! host struct keeps its own `#[injectable]` (which owns `Discoverable`), and
//! several decorated methods pool the provider's `#[inject]` dependencies.
//!
//! `#[on_event]` is a pure marker consumed here — it is not registered as a
//! proc-macro attribute, so writing it outside a `#[listeners]` impl block
//! fails the same way `#[get]` outside `#[routes]` does.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{FnArg, ImplItem, ItemImpl, PatType, ReturnType, Type, parse_macro_input};

use nest_rs_codegen::impl_self_ident;

pub(crate) fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[listeners] takes no arguments; tag methods with `#[on_event]`",
        )
        .to_compile_error()
        .into();
    }

    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();
    let provider_ident = match impl_self_ident(&self_ty, "#[listeners]") {
        Ok(ident) => ident,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_name = provider_ident.to_string();
    let provider_snake = to_snake(&provider_name);

    let mut emissions: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let attr_idx = method
            .attrs
            .iter()
            .position(|attr| attr.path().is_ident("on_event"));
        let Some(idx) = attr_idx else { continue };
        let attr = method.attrs.remove(idx);

        if attr.meta.require_path_only().is_err() {
            return syn::Error::new_spanned(
                attr,
                "#[on_event] takes no arguments; the event is read from the method's \
                 second parameter (e.g. `async fn on_x(&self, event: PointsAwarded)`)",
            )
            .to_compile_error()
            .into();
        }

        if method.sig.asyncness.is_none() {
            return syn::Error::new_spanned(&method.sig, "#[on_event] methods must be `async fn`")
                .to_compile_error()
                .into();
        }

        if !matches!(method.sig.output, ReturnType::Default) {
            return syn::Error::new_spanned(
                &method.sig.output,
                "#[on_event] methods are fire-and-forget — return `()` and handle errors \
                 inside the method (push a failed job to the queue, log, etc.)",
            )
            .to_compile_error()
            .into();
        }

        let event_ty = match extract_event_type(method) {
            Ok(ty) => ty,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.to_string();
        let qualified_name = format!("{provider_name}::{method_name}");
        let method_snake = to_snake(&method_name);
        let wire_ident =
            format_ident!("__nestrs_listener_wire_{}_{}", provider_snake, method_snake);

        emissions.push(quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #wire_ident(
                __container: &::nest_rs_core::Container,
                __bus: &::nest_rs_events::EventBus,
            ) {
                let __provider = ::nest_rs_core::Container::get::<#self_ty>(__container)
                    .expect(::std::concat!(
                        "listeners provider `",
                        #provider_name,
                        "` is not registered — add it to a reachable module's \
                         `providers = [...]`",
                    ));
                __bus.subscribe::<#event_ty, _, _>(move |__event| {
                    let __provider = ::std::sync::Arc::clone(&__provider);
                    async move {
                        <#self_ty>::#method_ident(&__provider, __event).await
                    }
                });
            }

            ::nest_rs_core::inventory::submit! {
                ::nest_rs_events::ListenerMethod {
                    name: #qualified_name,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    wire: #wire_ident,
                }
            }
        });
    }

    let out = quote! {
        #item
        #(#emissions)*
    };
    out.into()
}

/// Extract the second parameter's type — the event payload.
fn extract_event_type(method: &syn::ImplItemFn) -> syn::Result<Type> {
    let mut iter = method.sig.inputs.iter();
    match iter.next() {
        Some(FnArg::Receiver(_)) => {}
        Some(other) => {
            return Err(syn::Error::new(
                other.span(),
                "an `#[on_event]` method must take `&self` as its first argument",
            ));
        }
        None => {
            return Err(syn::Error::new(
                method.sig.span(),
                "an `#[on_event]` method must take `&self` and one event argument",
            ));
        }
    }
    let Some(arg) = iter.next() else {
        return Err(syn::Error::new(
            method.sig.span(),
            "an `#[on_event]` method needs an event argument: \
             `async fn(&self, event: T)`",
        ));
    };
    if iter.next().is_some() {
        return Err(syn::Error::new(
            method.sig.span(),
            "an `#[on_event]` method takes exactly one event argument — \
             extra dependencies belong on the host struct as `#[inject]` fields",
        ));
    }
    match arg {
        FnArg::Typed(PatType { ty, .. }) => Ok((**ty).clone()),
        FnArg::Receiver(r) => Err(syn::Error::new(
            r.span(),
            "an `#[on_event]` method takes exactly one `&self` receiver",
        )),
    }
}

fn to_snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, ch) in camel.chars().enumerate() {
        if ch.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
