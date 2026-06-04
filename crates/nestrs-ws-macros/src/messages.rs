//! `#[messages]` — bind a `#[gateway]` impl block's `#[subscribe_message]`
//! methods to WebSocket events; emit the `Gateway` dispatcher and the
//! `Discoverable` impl that self-mounts on the HTTP transport.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Path, ReturnType, Type, parse_macro_input,
};

use nestrs_codegen::{injected_method_with_layers, layer_inject_keys};

use crate::attr::take_use_attr;

pub(crate) fn messages(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    let mut arms: Vec<TokenStream2> = Vec::new();
    let mut guard_inserts: Vec<TokenStream2> = Vec::new();
    // Folded into `Discoverable::injected` for the access-graph check, same
    // as HTTP per-route layer keys.
    let mut message_guards: Vec<Path> = Vec::new();
    let mut on_connect: Option<TokenStream2> = None;
    let mut on_disconnect: Option<TokenStream2> = None;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // `#[on_connect]` / `#[on_disconnect]` — `OnGatewayConnection` /
        // `OnGatewayDisconnect` analogs. Consume the marker and emit a
        // `Gateway` trait override delegating to the method.
        if strip_marker(method, "on_connect") {
            on_connect = Some(match hook_override("on_connect", method) {
                Ok(tokens) => tokens,
                Err(err) => return err.to_compile_error().into(),
            });
            continue;
        }
        if strip_marker(method, "on_disconnect") {
            on_disconnect = Some(match hook_override("on_disconnect", method) {
                Ok(tokens) => tokens,
                Err(err) => return err.to_compile_error().into(),
            });
            continue;
        }

        let Some(idx) = method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("subscribe_message"))
        else {
            continue;
        };

        let attr = method.attrs.remove(idx);
        let event: LitStr = match attr.parse_args() {
            Ok(e) => e,
            Err(err) => return err.to_compile_error().into(),
        };

        let guards = match take_use_attr(&mut method.attrs, "use_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        if !guards.is_empty() {
            guard_inserts.push(guard_insert(&event, &guards));
            message_guards.extend(guards);
        }

        let method_name = method.sig.ident.clone();

        // Owned arg = the payload (deserialized from the envelope's `data`);
        // `&`-reference arg = the connected `WsClient`. At most one of each —
        // same owned/reference split a `#[field]` resolver uses.
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

        let return_kind = classify_return(&method.sig.output);

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

        let arm_body = match return_kind {
            ReturnKind::Unit => quote! {
                { #call };
                ::nestrs_ws::WsReply::None
            },
            ReturnKind::Value => quote! {
                let __ret = { #call };
                ::nestrs_ws::WsReply::reply(&__ret)
            },
            ReturnKind::ResultUnit => quote! {
                match { #call } {
                    ::core::result::Result::Ok(()) => ::nestrs_ws::WsReply::None,
                    ::core::result::Result::Err(__err) => {
                        // Debug for the server log, Display for the wire —
                        // handler's error type owns Display wire-safety.
                        ::nestrs_ws::tracing::warn!(
                            target: "nestrs::ws",
                            event = #event,
                            error = ?__err,
                            "subscribe_message handler returned Err",
                        );
                        ::nestrs_ws::WsReply::error(::std::format!("{}", __err))
                    }
                }
            },
            ReturnKind::Result => quote! {
                match { #call } {
                    ::core::result::Result::Ok(__ret) => ::nestrs_ws::WsReply::reply(&__ret),
                    ::core::result::Result::Err(__err) => {
                        ::nestrs_ws::tracing::warn!(
                            target: "nestrs::ws",
                            event = #event,
                            error = ?__err,
                            "subscribe_message handler returned Err",
                        );
                        ::nestrs_ws::WsReply::error(::std::format!("{}", __err))
                    }
                }
            },
        };

        arms.push(quote! { #event => { #arm_body } });
    }

    let message_guard_keys = layer_inject_keys(message_guards.iter());
    let injected_method = injected_method_with_layers(&self_ty, &message_guard_keys);

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

            #on_connect
            #on_disconnect
        }

        impl ::nestrs_core::Discoverable for #self_ty {
            #injected_method

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                // A namespaced gateway self-provides its own `WsServer<Ns>`;
                // `Global` comes from `WsModule` (no-op here).
                let builder = <#self_ty>::__nestrs_provide_registry(builder);
                // Self-mount on HTTP: a WS upgrade is an HTTP `GET`, so a
                // gateway is just another `HttpEndpointMeta` at boot.
                builder.attach_meta::<#self_ty, ::nestrs_http::HttpEndpointMeta>(
                    ::nestrs_http::HttpEndpointMeta::new(
                        <#self_ty>::PATH,
                        "ws",
                        |__container, __route| {
                            let __gw = ::std::sync::Arc::new(
                                <#self_ty>::from_container(__container),
                            );
                            let __server = <#self_ty>::__nestrs_registry(__container);
                            let mut __guards = ::nestrs_ws::MessageGuardTable::new();
                            #(#guard_inserts)*
                            let __ctx = ::nestrs_core::Container::get_dyn::<
                                dyn ::nestrs_ws::SocketContext,
                            >(__container);
                            let __ep = ::nestrs_ws::gateway_endpoint(__gw, __server, __guards, __ctx);
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

enum ReturnKind {
    Unit,
    Value,
    ResultUnit,
    Result,
}

fn classify_return(output: &ReturnType) -> ReturnKind {
    let ty = match output {
        ReturnType::Default => return ReturnKind::Unit,
        ReturnType::Type(_, ty) => ty.as_ref(),
    };
    if let Type::Tuple(t) = ty
        && t.elems.is_empty()
    {
        return ReturnKind::Unit;
    }
    let Type::Path(tp) = ty else {
        return ReturnKind::Value;
    };
    let Some(last) = tp.path.segments.last() else {
        return ReturnKind::Value;
    };
    if last.ident != "Result" {
        return ReturnKind::Value;
    }
    if let syn::PathArguments::AngleBracketed(args) = &last.arguments
        && let Some(syn::GenericArgument::Type(Type::Tuple(t))) = args.args.first()
        && t.elems.is_empty()
    {
        return ReturnKind::ResultUnit;
    }
    ReturnKind::Result
}

fn strip_marker(method: &mut ImplItemFn, ident: &str) -> bool {
    if let Some(pos) = method.attrs.iter().position(|a| a.path().is_ident(ident)) {
        method.attrs.remove(pos);
        true
    } else {
        false
    }
}

/// Emit the `Gateway` override for `on_connect` / `on_disconnect` delegating
/// to the user method. The hook may declare an optional `&WsClient` parameter.
fn hook_override(hook: &str, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let hook_ident = syn::Ident::new(hook, proc_macro2::Span::call_site());
    let method_name = method.sig.ident.clone();

    let mut takes_client = false;
    for arg in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(pt) = arg else { continue };
        if !matches!(pt.ty.as_ref(), Type::Reference(_)) {
            return Err(syn::Error::new_spanned(
                &pt.ty,
                format!("a #[{hook}] hook takes only an optional `&WsClient` parameter"),
            ));
        }
        if takes_client {
            return Err(syn::Error::new_spanned(
                &pt.ty,
                format!("a #[{hook}] hook takes at most one `&WsClient` parameter"),
            ));
        }
        takes_client = true;
    }

    let body = if takes_client {
        quote! { self.#method_name(__client).await; }
    } else {
        quote! {
            let _ = __client;
            self.#method_name().await;
        }
    };
    Ok(quote! {
        async fn #hook_ident(&self, __client: &::nestrs_ws::WsClient) {
            #body
        }
    })
}

/// Build the `__guards.insert("event", vec![…])` for a guarded handler.
/// First listed runs first.
fn guard_insert(event: &LitStr, paths: &[Path]) -> TokenStream2 {
    let resolved = paths.iter().map(|p| {
        quote! {
            {
                let __g: ::std::sync::Arc<dyn ::nestrs_ws::MessageGuard> =
                    ::nestrs_core::Container::get::<#p>(__container).expect(concat!(
                        "#[use_guards] message guard `",
                        stringify!(#p),
                        "` is not registered — add it to a module's providers"
                    ));
                __g
            }
        }
    });
    quote! {
        __guards.insert(#event, ::std::vec![ #(#resolved),* ]);
    }
}
