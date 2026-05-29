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

        // Classify the parameters after `&self`, preserving their declared order
        // for the call. An **owned** parameter is the message payload
        // (deserialized from the envelope's `data`); a `&`-reference parameter is
        // the connected `WsClient` (the `@ConnectedSocket` analog) — the same
        // owned-vs-reference split a `#[field]` resolver uses to tell a GraphQL
        // argument from an injected `&DataLoader`. At most one of each.
        let mut payload_ty: Option<&Type> = None;
        let mut takes_client = false;
        let mut call_args: Vec<TokenStream2> = Vec::new();
        let mut arity_error: Option<syn::Error> = None;
        for arg in method.sig.inputs.iter().skip(1) {
            let FnArg::Typed(pt) = arg else { continue };
            if matches!(pt.ty.as_ref(), Type::Reference(_)) {
                if takes_client {
                    arity_error = Some(syn::Error::new_spanned(
                        &pt.ty,
                        "a #[subscribe_message] handler takes at most one `&WsClient` parameter",
                    ));
                    break;
                }
                takes_client = true;
                call_args.push(quote! { __client });
            } else {
                if payload_ty.is_some() {
                    arity_error = Some(syn::Error::new_spanned(
                        &pt.ty,
                        "a #[subscribe_message] handler takes at most one payload parameter \
                         (deserialized from the message's `data`)",
                    ));
                    break;
                }
                payload_ty = Some(pt.ty.as_ref());
                call_args.push(quote! { __payload });
            }
        }
        if let Some(err) = arity_error {
            return err.to_compile_error().into();
        }

        // A `()`/no return sends no reply; any other return is serialized back
        // to the client under the same event name.
        let returns_unit = match &method.sig.output {
            ReturnType::Default => true,
            ReturnType::Type(_, ty) => matches!(ty.as_ref(), Type::Tuple(t) if t.elems.is_empty()),
        };

        let deser = match payload_ty {
            Some(ty) => quote! {
                let __payload: #ty = match ::nestrs_ws::serde_json::from_value(__data) {
                    ::core::result::Result::Ok(__p) => __p,
                    ::core::result::Result::Err(__e) => {
                        return ::nestrs_ws::WsReply::error(::std::format!(
                            "invalid payload for `{}`: {}", #event, __e,
                        ));
                    }
                };
            },
            None => quote! {},
        };
        let call = quote! {
            #deser
            self.#method_name(#(#call_args),*).await
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
                __client: &::nestrs_ws::WsClient,
                __event: &str,
                __data: ::nestrs_ws::serde_json::Value,
            ) -> ::nestrs_ws::WsReply {
                let _ = &__data;
                let _ = __client;
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
                            // The shared connection registry every connection of
                            // this gateway registers into. Resolved from the
                            // container (the `Container::get` escape hatch), so
                            // the app must import `WsModule` to provide it.
                            let __server = ::nestrs_core::Container::get::<::nestrs_ws::WsServer>(
                                __container,
                            )
                            .expect(
                                "WebSocket gateway requires the connection registry — \
                                 add `WsModule` to a module's `imports`",
                            );
                            let __ep = ::nestrs_ws::gateway_endpoint(__gw, __server);
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
