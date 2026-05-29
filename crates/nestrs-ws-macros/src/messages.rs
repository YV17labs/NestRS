//! `#[messages]` — bind a `#[gateway]` impl block's `#[subscribe_message]`
//! methods to incoming WebSocket events, emitting the `Gateway` dispatcher and
//! the `Discoverable` impl that self-mounts the gateway on the HTTP transport.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, LitStr, ReturnType, Type};

pub(crate) fn messages(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    let mut arms: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let Some(idx) = method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("subscribe_message"))
        else {
            continue;
        };

        // `#[subscribe_message("event")]` — consume it (so it never reaches the
        // compiler as an unknown attribute) and read the event name.
        let attr = method.attrs.remove(idx);
        let event: LitStr = match attr.parse_args() {
            Ok(e) => e,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_name = method.sig.ident.clone();

        // The handler's owned parameter (after `&self`) is the message payload,
        // deserialized from the envelope's `data`. Zero or one is allowed.
        let mut payload_tys = method
            .sig
            .inputs
            .iter()
            .skip(1)
            .filter_map(|arg| match arg {
                FnArg::Typed(pt) => Some(pt.ty.as_ref()),
                FnArg::Receiver(_) => None,
            });
        let payload_ty = payload_tys.next();
        if payload_tys.next().is_some() {
            return syn::Error::new_spanned(
                &method.sig,
                "a #[subscribe_message] handler takes at most one payload parameter \
                 (deserialized from the message's `data`)",
            )
            .to_compile_error()
            .into();
        }

        // A `()`/no return sends no reply; any other return is serialized back
        // to the client under the same event name.
        let returns_unit = match &method.sig.output {
            ReturnType::Default => true,
            ReturnType::Type(_, ty) => matches!(ty.as_ref(), Type::Tuple(t) if t.elems.is_empty()),
        };

        let call = match payload_ty {
            Some(ty) => quote! {
                let __payload: #ty = match ::nestrs_ws::serde_json::from_value(__data) {
                    ::core::result::Result::Ok(__p) => __p,
                    ::core::result::Result::Err(__e) => {
                        return ::nestrs_ws::WsReply::error(::std::format!(
                            "invalid payload for `{}`: {}", #event, __e,
                        ));
                    }
                };
                self.#method_name(__payload).await
            },
            None => quote! { self.#method_name().await },
        };

        let arm_body = if returns_unit {
            quote! {
                { #call };
                ::nestrs_ws::WsReply::None
            }
        } else {
            quote! {
                let __ret = { #call };
                ::nestrs_ws::WsReply::reply(&__ret)
            }
        };

        arms.push(quote! { #event => { #arm_body } });
    }

    quote! {
        #item

        #[::nestrs_ws::async_trait]
        impl ::nestrs_ws::Gateway for #self_ty {
            async fn dispatch(
                &self,
                __event: &str,
                __data: ::nestrs_ws::serde_json::Value,
            ) -> ::nestrs_ws::WsReply {
                let _ = &__data;
                match __event {
                    #(#arms)*
                    __other => ::nestrs_ws::WsReply::unknown(__other),
                }
            }
        }

        impl ::nestrs_core::Discoverable for #self_ty {
            // The gateway is built at mount time (like a controller), so
            // `dependencies` (register ordering) stays empty; `injected` reports
            // its `#[inject]` keys for the access-graph check.
            fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
                <#self_ty>::__nestrs_injected()
            }

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                // Self-mount on the HTTP transport's route tree: the WebSocket
                // upgrade is an HTTP `GET`, so a gateway is just another
                // `HttpEndpointMeta` the transport mounts at boot — no `main.rs`
                // wiring, exactly like a GraphQL or OpenAPI endpoint.
                builder.attach_meta::<#self_ty, ::nestrs_http::HttpEndpointMeta>(
                    ::nestrs_http::HttpEndpointMeta::new(
                        <#self_ty>::PATH,
                        "ws",
                        |__container, __route| {
                            let __gw = ::std::sync::Arc::new(
                                <#self_ty>::from_container(__container),
                            );
                            let __ep = ::nestrs_ws::gateway_endpoint(__gw);
                            let __ep = <#self_ty>::__nestrs_gateway_layers(__container, __ep);
                            __route.at(<#self_ty>::PATH, __ep)
                        },
                    ),
                )
            }
        }
    }
    .into()
}
