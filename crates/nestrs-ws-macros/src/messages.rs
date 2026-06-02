//! `#[messages]` — bind a `#[gateway]` impl block's `#[subscribe_message]`
//! methods to incoming WebSocket events, emitting the `Gateway` dispatcher (with
//! any `#[on_connect]`/`#[on_disconnect]` lifecycle hooks) and the `Discoverable`
//! impl that self-mounts the gateway on the HTTP transport.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse_macro_input, FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Path, ReturnType, Type,
};

use nestrs_codegen::{injected_method_with_layers, layer_inject_keys};

use crate::attr::take_use_attr;

pub(crate) fn messages(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    let mut arms: Vec<TokenStream2> = Vec::new();
    // `event => vec![guard, …]` inserts the mount closure runs to build the
    // per-message guard table from the container.
    let mut guard_inserts: Vec<TokenStream2> = Vec::new();
    // Every per-message guard path, gathered for the access-graph check (folded
    // into `Discoverable::injected` below, like the HTTP per-route layers).
    let mut message_guards: Vec<Path> = Vec::new();
    let mut on_connect: Option<TokenStream2> = None;
    let mut on_disconnect: Option<TokenStream2> = None;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // Connection lifecycle hooks (`#[on_connect]` / `#[on_disconnect]`) — the
        // `OnGatewayConnection` / `OnGatewayDisconnect` analogs. Consume the inert
        // attribute and emit a `Gateway` trait override delegating to the method.
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

        // `#[subscribe_message("event")]` — consume it (so it never reaches the
        // compiler as an unknown attribute) and read the event name.
        let attr = method.attrs.remove(idx);
        let event: LitStr = match attr.parse_args() {
            Ok(e) => e,
            Err(err) => return err.to_compile_error().into(),
        };

        // `#[use_guards(GuardA, GuardB)]` beside the verb — per-message guards,
        // resolved from the container at mount into the event's table entry. The
        // attribute is consumed here, exactly like the HTTP verb's `#[use_guards]`.
        let guards = match take_use_attr(&mut method.attrs, "use_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        if !guards.is_empty() {
            guard_inserts.push(guard_insert(&event, &guards));
            message_guards.extend(guards);
        }

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

        // Classify the return to pick the dispatch shape: a `()` (or no return)
        // sends nothing, a plain `T` is serialized as the reply, a `Result<T, E>`
        // unwraps — `Ok(T)` becomes the reply (or `None` when `T == ()`) and
        // `Err(E)` becomes an error frame plus a `warn` log. Detection is by the
        // *path*'s last segment being `Result`; aliasing a different type as
        // `Result` is out of scope.
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
                        // `?__err` (Debug) captures the structured error for the
                        // server log; `{}` (Display) is what ships on the wire —
                        // the handler's error type is responsible for keeping its
                        // Display wire-safe (no `#[error(transparent)]` over an
                        // ORM/sqlx error). See the discipline note in
                        // `nestrs_ws::lib.rs`.
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
                        // See the note on `ReturnKind::ResultUnit` above —
                        // Debug for the log, Display for the wire.
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
            // The gateway is built at mount time (like a controller), so
            // `dependencies` (register ordering) stays empty; `injected` reports
            // its `#[inject]` keys, its connection-level guards (from the inherent
            // fn `#[gateway]` emits) and the per-message guards gathered here for
            // the access-graph check.
            #injected_method

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                // A namespaced gateway self-provides its own `WsServer<Ns>`; the
                // default `Global` registry comes from `WsModule` (no-op here).
                let builder = <#self_ty>::__nestrs_provide_registry(builder);
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
                            // This gateway's connection registry (its namespace
                            // baked into the helper `#[gateway]` emitted).
                            let __server = <#self_ty>::__nestrs_registry(__container);
                            // The per-message guard table, resolved from the
                            // container once and shared across every connection.
                            let mut __guards = ::nestrs_ws::MessageGuardTable::new();
                            #(#guard_inserts)*
                            // The optional ambient-data bridge (the executor +
                            // ability re-installer), resolved once at mount. With
                            // none bound, the connection loop dispatches without
                            // any ambient context.
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

/// What a `#[subscribe_message]` handler's return type looks like — drives the
/// shape of the generated dispatch arm.
enum ReturnKind {
    /// `()` (or no return). Send nothing.
    Unit,
    /// A plain `T`. Serialize as the reply.
    Value,
    /// `Result<(), E>`. `Ok(())` sends nothing; `Err(e)` becomes an error frame.
    ResultUnit,
    /// `Result<T, E>` with `T != ()`. `Ok(t)` is serialized as the reply; `Err(e)`
    /// becomes an error frame (and a `warn` log on `nestrs::ws`).
    Result,
}

fn classify_return(output: &ReturnType) -> ReturnKind {
    let ty = match output {
        ReturnType::Default => return ReturnKind::Unit,
        ReturnType::Type(_, ty) => ty.as_ref(),
    };
    if let Type::Tuple(t) = ty {
        if t.elems.is_empty() {
            return ReturnKind::Unit;
        }
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
    if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
        if let Some(syn::GenericArgument::Type(Type::Tuple(t))) = args.args.first() {
            if t.elems.is_empty() {
                return ReturnKind::ResultUnit;
            }
        }
    }
    ReturnKind::Result
}

/// Remove a bare marker attribute (`#[on_connect]`) from a method, returning
/// whether it was present.
fn strip_marker(method: &mut ImplItemFn, ident: &str) -> bool {
    if let Some(pos) = method.attrs.iter().position(|a| a.path().is_ident(ident)) {
        method.attrs.remove(pos);
        true
    } else {
        false
    }
}

/// Emit the `Gateway` trait override for a lifecycle hook (`on_connect` /
/// `on_disconnect`) delegating to the user method. The hook takes `&self` and an
/// optional single `&WsClient` parameter — passed through when declared.
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

    // Pass the client through only when the hook declared it; otherwise bind it
    // to `_` so the override's parameter never warns as unused.
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

/// Build the `__guards.insert("event", vec![…]);` statement for a guarded
/// handler: each path is resolved from the container and coerced to
/// `Arc<dyn MessageGuard>`. First listed runs first (insertion order preserved).
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
