//! `#[messages]` — bind a `#[gateway]` impl block's `#[subscribe_message]`
//! methods to WebSocket events; emit the `Gateway` dispatcher and the
//! `Discoverable` impl that self-mounts on the HTTP transport.
//!
//! Each `#[subscribe_message]` handler runs through the Layer System: the
//! global guard chain (from `App::builder().use_guards_global(...)`) is
//! merged with per-message `#[use_guards]`, deduped by `TypeId`, then
//! driven via [`EventLayerTable`] at dispatch in declaration order. The
//! chain is composed **once at gateway mount** and frozen for the rest of
//! the process — no per-message container lookup.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Path, ReturnType, Type, parse_macro_input,
};

use nest_rs_codegen::{injected_method_with_layers, layer_inject_keys};

use crate::attr::{take_flag_attr, take_use_attr};

pub(crate) fn messages(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    let mut arms: Vec<TokenStream2> = Vec::new();
    let mut chain_inserts: Vec<TokenStream2> = Vec::new();
    // Folded into `Discoverable::injected` for the access-graph check, same
    // as HTTP per-route layer keys.
    let mut all_message_layers: Vec<Path> = Vec::new();
    let mut on_connect: Option<TokenStream2> = None;
    let mut on_disconnect: Option<TokenStream2> = None;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

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
        let force_guards = match take_use_attr(&mut method.attrs, "force_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[public]` is consumed but not acted on at the framework level;
        // a WS guard that cares can read its own marker from socket state.
        let _is_public = take_flag_attr(&mut method.attrs, "public");
        all_message_layers.extend(guards.iter().cloned());
        all_message_layers.extend(force_guards.iter().cloned());

        chain_inserts.push(chain_insert(&event, &guards, &force_guards));

        let method_name = method.sig.ident.clone();

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
                let __payload: #ty = match ::nest_rs_ws::serde_json::from_value(__data) {
                    ::core::result::Result::Ok(__p) => __p,
                    ::core::result::Result::Err(__e) => {
                        return ::nest_rs_ws::WsReply::error(::std::format!(
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
                ::nest_rs_ws::WsReply::None
            },
            ReturnKind::Value => quote! {
                let __ret = { #call };
                ::nest_rs_ws::WsReply::reply(&__ret)
            },
            ReturnKind::ResultUnit => quote! {
                match { #call } {
                    ::core::result::Result::Ok(()) => ::nest_rs_ws::WsReply::None,
                    ::core::result::Result::Err(__err) => {
                        ::nest_rs_ws::tracing::warn!(
                            target: "nest_rs::ws",
                            event = #event,
                            error = ?__err,
                            "subscribe_message handler returned Err",
                        );
                        ::nest_rs_ws::WsReply::error(::std::format!("{}", __err))
                    }
                }
            },
            ReturnKind::Result => quote! {
                match { #call } {
                    ::core::result::Result::Ok(__ret) => ::nest_rs_ws::WsReply::reply(&__ret),
                    ::core::result::Result::Err(__err) => {
                        ::nest_rs_ws::tracing::warn!(
                            target: "nest_rs::ws",
                            event = #event,
                            error = ?__err,
                            "subscribe_message handler returned Err",
                        );
                        ::nest_rs_ws::WsReply::error(::std::format!("{}", __err))
                    }
                }
            },
        };

        arms.push(quote! { #event => { #arm_body } });
    }

    let message_guard_keys = layer_inject_keys(all_message_layers.iter());
    let injected_method = injected_method_with_layers(&self_ty, &message_guard_keys);

    quote! {
        #item

        #[::nest_rs_ws::async_trait]
        impl ::nest_rs_ws::Gateway for #self_ty {
            async fn dispatch(
                &self,
                __client: &::nest_rs_ws::WsClient,
                __event: &str,
                __data: ::nest_rs_ws::serde_json::Value,
            ) -> ::nest_rs_ws::WsReply {
                let _ = &__data;
                let _ = __client;
                match __event {
                    #(#arms)*
                    __other => ::nest_rs_ws::WsReply::unknown(__other),
                }
            }

            #on_connect
            #on_disconnect
        }

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_method

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                // A namespaced gateway self-provides its own `WsServer<Ns>`;
                // `Global` comes from `WsModule` (no-op here).
                let builder = <#self_ty>::__nestrs_provide_registry(builder);
                // Self-mount on HTTP: a WS upgrade is an HTTP `GET`, so a
                // gateway is just another `HttpEndpointMeta` at boot.
                builder.attach_meta::<#self_ty, ::nest_rs_http::HttpEndpointMeta>(
                    ::nest_rs_http::HttpEndpointMeta::new(
                        <#self_ty>::PATH,
                        "ws",
                        |__container, __route| {
                            let __gw = ::std::sync::Arc::new(
                                <#self_ty>::from_container(__container),
                            );
                            let __server = <#self_ty>::__nestrs_registry(__container);
                            let mut __chains = ::nest_rs_ws::EventLayerTable::new();
                            // Resolve every globally-registered guard once — every
                            // event arm reuses the same vec to compose its chain.
                            let __global_guards: ::std::vec::Vec<(
                                ::core::any::TypeId,
                                &'static str,
                                ::std::sync::Arc<dyn ::nest_rs_guards::Guard>,
                            )> = match ::nest_rs_core::Container::get::<
                                ::nest_rs_guards::GuardSpecs,
                            >(__container) {
                                ::core::option::Option::Some(__specs) => __specs.0
                                    .iter()
                                    .filter_map(|__s| __s
                                        .resolve(__container)
                                        .map(|__g| (__s.type_id, __s.name, __g)))
                                    .collect(),
                                ::core::option::Option::None => ::std::vec![],
                            };
                            #(#chain_inserts)*
                            let __ctx = ::nest_rs_core::Container::get_dyn::<
                                dyn ::nest_rs_ws::SocketContext,
                            >(__container);
                            let __ep = ::nest_rs_ws::gateway_endpoint(__gw, __server, __chains, __ctx);
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
        async fn #hook_ident(&self, __client: &::nest_rs_ws::WsClient) {
            #body
        }
    })
}

/// Build the chain-insert for one `#[subscribe_message]` event.
///
/// The chain is `global + method_guards`, deduped by `TypeId` (broadest
/// wins). `#[force_guards]` lets a per-message guard replay even when the
/// same `TypeId` is global.
fn chain_insert(event: &LitStr, method_guards: &[Path], force_guards: &[Path]) -> TokenStream2 {
    let method_spec_entries = method_guards.iter().map(|p| {
        quote! {
            ::nest_rs_guards::layer_chain::ResolvedLayer {
                type_id: ::core::any::TypeId::of::<#p>(),
                name: ::core::any::type_name::<#p>(),
                source: ::nest_rs_guards::layer_chain::LayerSource::Method,
                layer: ::nest_rs_core::Container::get::<#p>(__container).expect(concat!(
                    "#[use_guards] WS message guard `",
                    stringify!(#p),
                    "` is not registered — add it to a module's providers"
                )) as ::std::sync::Arc<dyn ::nest_rs_guards::Guard>,
            }
        }
    });
    let force_typeids = force_guards.iter().map(|p| {
        quote! { ::core::any::TypeId::of::<#p>() }
    });
    quote! {
        {
            let __global: ::std::vec::Vec<
                ::nest_rs_guards::layer_chain::ResolvedLayer<dyn ::nest_rs_guards::Guard>
            > = __global_guards
                .iter()
                .map(|(__tid, __name, __arc)| ::nest_rs_guards::layer_chain::ResolvedLayer {
                    type_id: *__tid,
                    name: __name,
                    source: ::nest_rs_guards::layer_chain::LayerSource::Global,
                    layer: ::std::sync::Arc::clone(__arc),
                })
                .collect();
            let __method: ::std::vec::Vec<
                ::nest_rs_guards::layer_chain::ResolvedLayer<dyn ::nest_rs_guards::Guard>
            > = ::std::vec![#(#method_spec_entries),*];
            let __force: ::std::vec::Vec<::core::any::TypeId> = ::std::vec![#(#force_typeids),*];
            let __label = ::std::format!("ws {}", #event);
            let __chain = ::nest_rs_guards::layer_chain::compose_chain::<dyn ::nest_rs_guards::Guard>(
                __global,
                ::std::vec![],
                __method,
                &__force,
                &__label,
            );
            let __ws_chain: ::std::vec::Vec<
                ::std::sync::Arc<dyn ::nest_rs_ws::WsMessageCheck>
            > = __chain
                .into_iter()
                .map(|__e| {
                    let __wrapped = ::nest_rs_guards::GuardAsWsLayer::new(
                        ::std::sync::Arc::clone(&__e.layer),
                        __e.type_id,
                        __e.name,
                    );
                    ::std::sync::Arc::new(__wrapped)
                        as ::std::sync::Arc<dyn ::nest_rs_ws::WsMessageCheck>
                })
                .collect();
            __chains.insert(#event, __ws_chain);
        }
    }
}
