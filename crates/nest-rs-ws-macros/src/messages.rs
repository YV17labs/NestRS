//! `#[messages]` — bind a `#[gateway]` impl block's `#[subscribe_message]`
//! methods to WebSocket events; emit the `Gateway` dispatcher and the
//! `Discoverable` impl that self-mounts on the HTTP transport. Each event's
//! guard chain is composed once at mount.

use nest_rs_codegen::__private::pair;
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{FnArg, ImplItem, ImplItemFn, LitStr, Path, ReturnType, Type};

use nest_rs_codegen::__private::layers::{LayerScope, Layers, take_layers};
use nest_rs_codegen::__private::{
    Collision, Conditional, DispatchKeys, HandlerDeclaration, HandlerSite, HostBorrow, HostMeta,
    PipeWrapper, Posture, PostureRules, SitePosture, await_if_async, cfg_attrs, concrete_signature,
    declared_handler, force_guard_typeids, guard_capability_bounds, handler_const, handler_ref,
    impl_self_ident, injected_methods_with_layers, injected_per_request_method, layer_deps,
    one_role_per_method, pipe_wrapper, reject_meta_on_operations, returns_unit, scoped_specs,
    shared_receiver, takes_value, ungrouped_expr,
};
use nest_rs_codegen::take_flag_attr;

/// WS's half of the shared posture grammar, mandatory per message.
const POSTURE: PostureRules = PostureRules {
    operation: "#[subscribe_message]",
    public_means: "no gate and no mask — the guards bound on the gateway and beside \
                   the message still run, and the connection's upgrade already \
                   authenticated it",
    transport: "WebSockets",
    bind_unsupported_because: "a message takes one payload value, not the named id argument \
                               the binding reads. Keep `#[authorize(Action, Entity)]` and load \
                               the subject in the body with the service's `access`",
};

/// `#[subscribe_message("chat")]`'s one argument, the event name, read as a
/// string literal.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn event_name(attr: &syn::Attribute) -> syn::Result<LitStr> {
    let refused = |at: &dyn quote::ToTokens| {
        syn::Error::new_spanned(
            at,
            takes_value(
                "subscribe_message",
                None,
                "the event's name as a string literal, e.g. `#[subscribe_message(\"chat\")]`",
            ),
        )
    };
    let written: syn::Expr = attr.parse_args().map_err(|_| refused(attr))?;
    match ungrouped_expr(&written) {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(event),
            ..
        }) => Ok(event.clone()),
        other => Err(refused(other)),
    }
}

/// Split a `#[subscribe_message]` payload argument into the wire type and its
/// pipe: `Some((Some(pipe), inner))` for `Piped<P, T>`, `Some((None, inner))`
/// for `Valid<T>`, `None` for a plain payload.
fn ws_pipe_binding(ty: &Type) -> (Type, Option<(Option<Path>, Type)>) {
    match pipe_wrapper(ty) {
        Some(PipeWrapper::Piped { pipe, value }) => (value.clone(), Some((Some(pipe), value))),
        Some(PipeWrapper::Valid { value }) => (value.clone(), Some((None, value))),
        None => (ty.clone(), None),
    }
}

pub(crate) fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::WS
        .keep_item_on_refusal(written, expansion, &HELPERS, |item| {
            let self_ty = &item.self_ty;
            quote! {
                impl ::nest_rs_core::Discoverable for #self_ty {
                    fn register(
                        builder: ::nest_rs_core::ContainerBuilder,
                    ) -> ::nest_rs_core::ContainerBuilder {
                        builder
                    }
                }
            }
        })
        .into()
}

/// What `#[messages]` consumes off a method beside the layers and the posture.
const HELPERS: [&str; 3] = ["subscribe_message", "on_connect", "on_disconnect"];

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = pair::WS.reject_args(
        &TokenStream2::from(args),
        "a gateway's `path`, `version` and `namespace` are declared by",
    ) {
        return err.to_compile_error().into();
    }
    let mut item = match pair::WS.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    if let Err(err) = pair::WS
        .reject_host_layers(&item.attrs)
        .and_then(|()| reject_meta_on_operations(&pair::WS, &item.attrs))
    {
        return err.to_compile_error().into();
    }
    let self_ty = item.self_ty.clone();

    let host = match impl_self_ident(&self_ty, "#[messages]") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let gateway_name = LitStr::new(&host.to_string(), host.span());

    let mut arms: Vec<TokenStream2> = Vec::new();
    // The class gate of each `#[authorize]` message, for `Gateway::dispatch`.
    let mut gate_arms: Vec<TokenStream2> = Vec::new();
    let mut mounted_logs: Vec<TokenStream2> = Vec::new();
    let mut message_mounts: Vec<TokenStream2> = Vec::new();
    let mut declared_messages: Vec<TokenStream2> = Vec::new();
    let mut message_guards: Vec<(Vec<TokenStream2>, Path)> = Vec::new();
    let mut message_layers: Vec<(Vec<TokenStream2>, Path)> = Vec::new();
    let mut message_params: Vec<(Vec<TokenStream2>, Type)> = Vec::new();
    // Every hook override is emitted under its method's `#[cfg]`; two compiled in
    // are two definitions of one trait method, which rustc refuses.
    let mut hooks: Vec<TokenStream2> = Vec::new();
    let mut site_items: Vec<TokenStream2> = Vec::new();
    // A second declaration is refused by the macro without `#[cfg]`, and by
    // rustc through the marker when both are compiled in.
    let mut declared = DispatchKeys::new(
        "#[messages]",
        "a gateway dispatches each event, and each connection hook, to one method, so the \
         second would never run — fold the two bodies into one",
    );

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index = match one_role_per_method(
            "role",
            &method.attrs,
            &["subscribe_message", "on_connect", "on_disconnect"],
            "",
        ) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };
        if let Err(err) = shared_receiver(method, "#[messages]", &host, HostBorrow::Host)
            .and_then(|()| concrete_signature(method, "#[messages]"))
        {
            return err.to_compile_error().into();
        }
        let cfgs = cfg_attrs(&method.attrs);
        let hook = ["on_connect", "on_disconnect"]
            .into_iter()
            .find(|name| method.attrs[index].path().is_ident(name));
        let role_attr = method.attrs[index].clone();
        let role_span = role_attr.path().span();
        let (kind, identity, key) = match hook {
            Some(hook) => ("hook", hook.to_owned(), format!("#[{hook}]")),
            None => match event_name(&method.attrs[index]) {
                Ok(event) => (
                    "event",
                    event.value(),
                    format!("#[subscribe_message({:?})]", event.value()),
                ),
                Err(err) => return err.to_compile_error().into(),
            },
        };
        // A hook's own trait method is its marker; an event has no item, so it gets one.
        let collision = match hook {
            Some(_) => Collision::Item,
            None => Collision::Marker,
        };
        let refused = declared.declare(
            collision,
            kind,
            &identity,
            &key,
            &method.sig.ident,
            &cfgs,
            &role_attr,
        );
        if let Err(err) = refused {
            return err.to_compile_error().into();
        }
        if let Some(hook) = hook {
            let metas = match take_layers(&pair::WS, LayerScope::Hook, &mut method.attrs) {
                Ok(layers) => layers.meta,
                Err(err) => return err.to_compile_error().into(),
            };
            site_items.push(handler_const(&HandlerSite {
                unit: if hook == "on_connect" {
                    quote!(::nest_rs_ws::unit::CONNECT)
                } else {
                    quote!(::nest_rs_ws::unit::DISCONNECT)
                },
                self_ty: &self_ty,
                host_name: &host,
                method: &method.sig.ident,
                address: quote!(#identity),
                // A hook runs on a connection the upgrade's guards admitted.
                posture: SitePosture::Gated,
                host_meta: HostMeta::Struct,
                method_meta: &metas,
                cfgs: &cfgs,
            }));
            if let Err(err) = take_flag_attr(&mut method.attrs, hook) {
                return err.to_compile_error().into();
            }
            match hook_override(hook, method, role_span) {
                Ok(tokens) => hooks.push(quote! { #(#cfgs)* #tokens }),
                Err(err) => return err.to_compile_error().into(),
            }
            continue;
        }

        let attr = method.attrs.remove(index);
        let event = match event_name(&attr) {
            Ok(event) => event,
            Err(err) => return err.to_compile_error().into(),
        };
        mounted_logs.push(quote! {
            #(#cfgs)*
            ::nest_rs_ws::tracing::info!(
                target: ::nest_rs_ws::target::ROUTES,
                gateway = #gateway_name,
                path = __path.as_str(),
                event = #event,
                "mounted message",
            );
        });

        let Layers {
            guards,
            force_guards,
            pipes,
            interceptors,
            exception_filters,
            meta: metas,
        } = match take_layers(&pair::WS, LayerScope::Method, &mut method.attrs) {
            Ok(layers) => layers,
            Err(err) => return err.to_compile_error().into(),
        };
        let posture = match POSTURE.take(method) {
            Ok(posture) => posture,
            Err(err) => return err.to_compile_error().into(),
        };
        site_items.push(handler_const(&HandlerSite {
            unit: quote!(::nest_rs_ws::unit::MESSAGE),
            self_ty: &self_ty,
            host_name: &host,
            method: &method.sig.ident,
            address: quote!(#identity),
            posture: posture.site_posture(),
            host_meta: HostMeta::Struct,
            method_meta: &metas,
            cfgs: &cfgs,
        }));
        message_guards.extend(guards.iter().map(|guard| (cfgs.clone(), guard.clone())));
        message_layers.extend(
            [&guards, &interceptors, &exception_filters, &pipes]
                .into_iter()
                .flatten()
                .map(|layer| (cfgs.clone(), layer.clone())),
        );

        let method_name = method.sig.ident.clone();
        let gate = match &posture {
            Posture::Authorize { action, entity, .. } => {
                let class_gate = quote! {
                    ::nest_rs_authz::class_gate::<#action, #entity>
                        as ::nest_rs_core::__private::ClassGate
                };
                let handler = handler_ref(&self_ty, &method_name);
                gate_arms.push(quote! {
                    #(#cfgs)*
                    #event => ::core::option::Option::Some((#handler, #class_gate)),
                });
                quote! { ::core::option::Option::Some(#class_gate) }
            }
            Posture::Public => quote! { ::core::option::Option::None },
        };
        let site = message_site(&MessageSite {
            self_ty: &self_ty,
            method: &method_name,
            interceptors: &interceptors,
            exception_filters: &exception_filters,
            guards: &guards,
            force_guards: &force_guards,
            gate,
            cfgs: &cfgs,
        });
        site_items.push(site.items);
        let site_ref = site.reference;
        let method_pipe_specs =
            scoped_specs(&pipes, quote!(dyn ::nest_rs_ws::__private::GlobalPipe));
        message_mounts.push(quote! {
            #(#cfgs)*
            __messages.mount(
                __container,
                #event,
                #site_ref,
                &__host_pipes,
                &#method_pipe_specs,
            );
        });
        declared_messages.push(quote! {
            #(#cfgs)*
            __messages.push(::nest_rs_ws::__private::DeclaredMessage {
                site: #site_ref,
                pipes: #method_pipe_specs,
            });
        });

        let mut payload_ty: Option<&Type> = None;
        let mut takes_client = false;
        let mut scoped: Vec<TokenStream2> = Vec::new();
        let mut call_args: Vec<TokenStream2> = Vec::new();
        let mut arity_error: Option<syn::Error> = None;
        for arg in method.sig.inputs.iter().skip(1) {
            let FnArg::Typed(pt) = arg else { continue };
            message_params.push((cfgs.clone(), pt.ty.as_ref().clone()));
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
            } else if is_scoped(pt.ty.as_ref()) {
                let ty = pt.ty.as_ref();
                let ident = format_ident!("__scoped_{}", scoped.len());
                scoped.push(quote! {
                    let #ident: #ty = match ::nest_rs_ws::__private::scoped_parameter() {
                        ::core::result::Result::Ok(__resolved) => __resolved,
                        ::core::result::Result::Err(__missing) => {
                            return ::core::result::Result::Ok(__missing);
                        }
                    };
                });
                call_args.push(quote! { #ident });
            } else {
                if payload_ty.is_some() {
                    arity_error = Some(syn::Error::new_spanned(
                        &pt.ty,
                        "a #[subscribe_message] handler takes at most one payload parameter \
                         (deserialized from the message's `data`) beside its `Scoped<T>` \
                         parameters",
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
            Some(ty) => {
                let (deser_ty, pipe) = ws_pipe_binding(ty);
                let wrap = match &pipe {
                    None => quote! { let __payload = __deser; },
                    Some((pipe_path, inner)) => {
                        let apply = match pipe_path {
                            Some(p) => {
                                quote!(::nest_rs_pipes::Piped::<#p, #inner>::apply(__deser))
                            }
                            None => quote!(::nest_rs_pipes::Valid::<#inner>::apply(__deser)),
                        };
                        quote! {
                            let __payload = match #apply {
                                ::core::result::Result::Ok(__p) => __p,
                                ::core::result::Result::Err(__e) => {
                                    return ::core::result::Result::Ok(
                                        ::nest_rs_ws::WsReply::pipe_error(#event, "payload", __e),
                                    );
                                }
                            };
                        }
                    }
                };
                quote! {
                    let __deser: #deser_ty = match ::nest_rs_ws::serde_json::from_value(__data) {
                        ::core::result::Result::Ok(__p) => __p,
                        ::core::result::Result::Err(__e) => {
                            return ::core::result::Result::Ok(
                                ::nest_rs_ws::WsReply::payload_error(#event, &__e),
                            );
                        }
                    };
                    #wrap
                }
            }
            None => quote! {},
        };

        let invoke = await_if_async(
            &method.sig,
            quote!(<#self_ty>::#method_name(self, #(#call_args),*)),
        );
        // The class gate ran before any of this: in the message's guard stage
        // on a socket, ahead of this call in `Gateway::dispatch`.
        let call = quote! {
            #(#scoped)*
            #deser
            #invoke
        };

        // The masked JSON ships, not a value round-tripped through the handler's
        // type (see `nest_rs_authz::ws::mask`). `__ret` is bound by the arms below.
        let reply = match &posture {
            Posture::Authorize {
                action,
                entity,
                unmasked: false,
            } => quote! {
                match ::nest_rs_authz::ws::masked_reply_for::<#action, #entity, _>(
                    #event, &__ret,
                ) {
                    ::core::result::Result::Ok(__masked) => {
                        ::core::result::Result::Ok(::nest_rs_ws::WsReply::Reply(__masked))
                    }
                    ::core::result::Result::Err(__failed) => {
                        ::core::result::Result::Ok(::nest_rs_ws::WsReply::Error(__failed))
                    }
                }
            },
            _ => quote! { ::core::result::Result::Ok(::nest_rs_ws::WsReply::reply(&__ret)) },
        };

        if posture.masks() && matches!(return_kind, ReturnKind::Value | ReturnKind::Unit) {
            return syn::Error::new_spanned(
                &method.sig.output,
                "a #[subscribe_message] declaring `#[authorize(Action, Entity)]` returns \
                 `Result<T, E>`, spelled literally: the reply shape is decided \
                 syntactically, so a `Result` behind an alias would be masked as the \
                 `Result` itself, and a fail-closed mask needs an error channel. Return a \
                 literal `Result`, or declare `#[authorize(Action, Entity, unmasked)]` and \
                 mask in the body",
            )
            .to_compile_error()
            .into();
        }

        // The report is built in library code, never by a call here: on an
        // `Infallible` error that call would be unreachable code. Its tiers
        // reach the last two through a borrow, so the binding is `mut`.
        let report = quote! {
            {
                #[allow(unused_imports)]
                use ::nest_rs_ws::{
                    ErrorReportChain as _, ErrorReportFallback as _, ErrorReportLocal as _,
                };
                #[allow(unused_mut)]
                let mut __report = __report;
                ::core::result::Result::Err(__report.into_failure())
            }
        };
        // Split by type through `ReplyValue`: an alias hides a `Result` from the
        // macro, not from method resolution.
        let split_then_reply = quote! {
            {
                #[allow(unused_imports)]
                use ::nest_rs_ws::ReplyValueFallback as _;
                match ::nest_rs_ws::ReplyValue(__ret).into_outcome() {
                    ::nest_rs_ws::ReplyOutcome::Value(__ret) => { #reply }
                    ::nest_rs_ws::ReplyOutcome::Failed(__report) => #report,
                }
            }
        };

        let arm_body = match return_kind {
            ReturnKind::Unit => quote! {
                { #call };
                ::core::result::Result::Ok(::nest_rs_ws::WsReply::None)
            },
            // Through `ReplyValue`, so an aliased `Result` never serializes its `Err`
            // into a success frame.
            ReturnKind::Value => quote! {
                let __ret = { #call };
                {
                    #[allow(unused_imports)]
                    use ::nest_rs_ws::ReplyValueFallback as _;
                    match ::nest_rs_ws::ReplyValue(__ret).into_outcome() {
                        ::nest_rs_ws::ReplyOutcome::Value(__ret) => #split_then_reply,
                        ::nest_rs_ws::ReplyOutcome::Failed(__report) => #report,
                    }
                }
            },
            ReturnKind::ResultUnit => quote! {
                match ::core::result::Result::map_err({ #call }, ::nest_rs_ws::ErrorReport::new) {
                    ::core::result::Result::Ok(()) => {
                        ::core::result::Result::Ok(::nest_rs_ws::WsReply::None)
                    }
                    ::core::result::Result::Err(__report) => #report,
                }
            },
            ReturnKind::Result => quote! {
                match ::core::result::Result::map_err({ #call }, ::nest_rs_ws::ErrorReport::new) {
                    ::core::result::Result::Ok(__ret) => #split_then_reply,
                    ::core::result::Result::Err(__report) => #report,
                }
            },
        };

        arms.push(quote! { #(#cfgs)* #event => { #arm_body } });
    }

    // `Guard::check` admits by default, so each per-message guard must declare
    // `GuardFor<WsView>`. Gateway-scope guards run on the HTTP upgrade instead.
    let capability_bounds = guard_capability_bounds(
        conditional(&message_guards),
        quote!(::nest_rs_guards::GuardFor<::nest_rs_ws::WsView>),
    );
    let host_name = host.unraw().to_string();
    let upgrade_handler = declared_handler(&HandlerDeclaration {
        unit: quote!(::nest_rs_ws::nest_rs_http::unit::REQUEST),
        host: quote!(#self_ty),
        host_name: &host_name,
        method: "",
        address: quote!(<#self_ty>::PATH),
        posture: SitePosture::Gated,
        meta: quote!(<#self_ty>::__nestrs_upgrade_meta),
    });
    let layers = layer_deps(conditional(&message_layers));
    let injected_methods = injected_methods_with_layers(&self_ty, &layers);
    // A provider a parameter resolves per message is checked like an `#[inject]`
    // one: an unimported `Scoped<T>` target fails the boot.
    let injected_per_request = injected_per_request_method(conditional(&message_params));
    let markers = declared.markers(&self_ty, &item.generics);
    let gate_lookup = if gate_arms.is_empty() {
        quote! {
            let _ = __event;
            ::core::option::Option::None
        }
    } else {
        quote! {
            #[allow(unreachable_patterns)]
            match __event {
                #(#gate_arms)*
                _ => ::core::option::Option::None,
            }
        }
    };

    quote! {
        #item

        impl #self_ty {
            #(#site_items)*

            /// The upgrade's metadata: the gateway's own, read by its guards.
            #[doc(hidden)]
            fn __nestrs_upgrade_meta() -> ::nest_rs_core::__private::MetaLevels {
                ::nest_rs_core::__private::MetaLevels {
                    host: <#self_ty>::__nestrs_host_meta(),
                    method: ::std::vec::Vec::new(),
                }
            }

            /// The site the gateway's upgrade runs as: an HTTP `GET` the gateway's
            /// guards gate, before any message.
            #[doc(hidden)]
            pub const __NESTRS_UPGRADE: &'static ::nest_rs_core::Handler = #upgrade_handler;

            /// The gateway's boot check, on the kernel's wiring step: its upgrade's
            /// guards, and each message's site and pipes, compose.
            #[doc(hidden)]
            fn __nestrs_wire(
                __container: &::nest_rs_core::Container,
            ) -> ::nest_rs_core::anyhow::Result<()> {
                let mut __messages: ::std::vec::Vec<::nest_rs_ws::__private::DeclaredMessage> =
                    ::std::vec::Vec::new();
                #(#declared_messages)*
                ::nest_rs_ws::__private::wire_gateway(
                    __container,
                    &<#self_ty>::__nestrs_mount_path(),
                    &<#self_ty>::__nestrs_edge_guard_specs(),
                    &<#self_ty>::__nestrs_gateway_pipe_specs(),
                    &__messages,
                )
            }
        }

        #capability_bounds

        #markers

        #[::nest_rs_ws::async_trait]
        impl ::nest_rs_ws::Gateway for #self_ty {
            async fn __nestrs_handle(
                &self,
                __client: &::nest_rs_ws::WsClient,
                __event: &str,
                __data: ::nest_rs_ws::serde_json::Value,
            ) -> ::core::result::Result<::nest_rs_ws::WsReply, ::nest_rs_ws::HandlerFailed> {
                let _ = &__data;
                let _ = __client;
                // Two arms for one event are refused — by the macro, or by rustc
                // at the marker — so the lint would only repeat that refusal.
                #[allow(unreachable_patterns)]
                match __event {
                    #(#arms)*
                    _ => ::core::result::Result::Ok(::nest_rs_ws::WsReply::unknown()),
                }
            }

            fn __nestrs_gate(
                &self,
                __event: &str,
            ) -> ::core::option::Option<(
                &'static ::nest_rs_core::Handler,
                ::nest_rs_core::__private::ClassGate,
            )> {
                #gate_lookup
            }

            #(#hooks)*
        }

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_methods

            #injected_per_request

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                // An upgraded socket outlives its HTTP connection: the `DetachedWork`
                // lets the transport tell each socket at shutdown and bound its window.
                let __sockets = ::nest_rs_ws::nest_rs_http::DetachedWork::new();
                let __carried = __sockets.clone();
                builder.attach_meta::<#self_ty, ::nest_rs_ws::nest_rs_http::HttpEndpointMeta>(
                    ::nest_rs_ws::nest_rs_http::__private::checks_origin(
                    ::nest_rs_ws::nest_rs_http::__private::guarded_by(
                    ::nest_rs_ws::nest_rs_http::HttpEndpointMeta::new(
                        <#self_ty>::__NESTRS_UPGRADE,
                        <#self_ty>::__nestrs_mount_path(),
                        "ws",
                        move |__container, __routes| {
                            let __path = <#self_ty>::__nestrs_mount_path();
                            #(#mounted_logs)*
                            let __gw = ::std::sync::Arc::new(
                                <#self_ty>::__nestrs_from_container(__container),
                            );
                            let __server = <#self_ty>::__nestrs_registry(__container);
                            let __host_pipes = <#self_ty>::__nestrs_gateway_pipe_specs();
                            let mut __messages = ::nest_rs_ws::__private::MessageTable::new();
                            #(#message_mounts)*
                            let __ep = ::nest_rs_ws::__private::gateway_endpoint(
                                __gw,
                                __server,
                                __messages,
                                __container,
                                ::core::clone::Clone::clone(&__carried),
                            );
                            __routes.route(
                                ::nest_rs_ws::nest_rs_http::Method::GET,
                                &__path,
                                __ep,
                            );
                        },
                    )
                    .owned_by(#gateway_name)
                    .runs_detached(__sockets),
                    <#self_ty>::__nestrs_edge_guard_specs(),
                    ),
                    ::nest_rs_ws::nest_rs_http::__private::OriginCheck::Upgrades,
                    ::nest_rs_ws::__private::refused_origin,
                ))
                // Every app importing the gateway checks it, whatever it serves.
                .provide_wiring("nest_rs::ws::gateways", <#self_ty>::__nestrs_wire)
            }
        }
    }
    .into()
}

/// One message's site, as `#[messages]` declares it.
struct MessageSite<'a> {
    self_ty: &'a Type,
    method: &'a syn::Ident,
    interceptors: &'a [Path],
    exception_filters: &'a [Path],
    guards: &'a [Path],
    force_guards: &'a [Path],
    /// The class gate's expression, an `Option<ClassGate>`.
    gate: TokenStream2,
    cfgs: &'a [TokenStream2],
}

/// The items declaring one message's site — its layers, its guard stage and
/// the `static Site` — and the path reading it back.
struct EmittedSite {
    items: TokenStream2,
    reference: TokenStream2,
}

/// The hidden associated items declaring `site` in the gateway's inherent
/// `impl`: the function building its layers, the one building its guard
/// stage, and the const holding its `&'static Site`. Every app-wide layer
/// runs around a message: it is accepted on its own, nested in none.
fn message_site(site: &MessageSite<'_>) -> EmittedSite {
    let MessageSite {
        self_ty,
        method,
        interceptors,
        exception_filters,
        guards,
        force_guards,
        gate,
        cfgs,
    } = site;
    let method_name = method.unraw().to_string();
    let lower = method_name.to_lowercase();
    let layers_fn = format_ident!("__nestrs_layers_{}", lower);
    let guards_fn = format_ident!("__nestrs_guards_{}", lower);
    let name = format_ident!("__NESTRS_SITE_{}", method_name.to_uppercase());
    let handler = handler_ref(self_ty, method);
    let method_guard_specs = scoped_specs(guards, quote!(dyn ::nest_rs_guards::Guard));
    let force_typeids = force_guard_typeids(force_guards);
    let items = quote! {
        #(#cfgs)*
        #[doc(hidden)]
        fn #layers_fn() -> ::nest_rs_core::__private::SiteLayers {
            ::nest_rs_core::__private::SiteLayers {
                host_interceptors: <#self_ty>::__nestrs_gateway_interceptor_specs(),
                method_interceptors: ::std::vec![
                    #(::nest_rs_core::interceptor::<#interceptors>()),*
                ],
                host_exception_filters: <#self_ty>::__nestrs_gateway_exception_filter_specs(),
                method_exception_filters: ::std::vec![
                    #(::nest_rs_core::exception_filter::<#exception_filters>()),*
                ],
            }
        }

        #(#cfgs)*
        #[doc(hidden)]
        fn #guards_fn(
            __nestrs_container: &::nest_rs_core::Container,
            __nestrs_handler: &::nest_rs_core::Handler,
        ) -> ::nest_rs_core::anyhow::Result<
            ::core::option::Option<
                ::std::sync::Arc<dyn ::nest_rs_core::__private::GuardStage>,
            >,
        > {
            ::nest_rs_ws::__private::message_guard_stage(
                __nestrs_container,
                __nestrs_handler,
                ::nest_rs_ws::__private::MessageGuards {
                    method: #method_guard_specs,
                    force: #force_typeids,
                    gate: #gate,
                },
            )
        }

        #(#cfgs)*
        #[doc(hidden)]
        pub const #name: &'static ::nest_rs_core::__private::Site = {
            static __NESTRS_SITE: ::nest_rs_core::__private::Site =
                ::nest_rs_core::__private::declare_site(
                    #handler,
                    ::nest_rs_core::__private::SiteDeclaration {
                        layers: <#self_ty>::#layers_fn,
                        global: ::nest_rs_core::__private::GlobalLayers::ALL,
                        guard_stage: ::core::option::Option::Some(
                            <#self_ty>::#guards_fn
                                as ::nest_rs_core::__private::GuardStageFactory,
                        ),
                        class_gate: ::core::option::Option::None,
                    },
                );
            &__NESTRS_SITE
        };
    };
    EmittedSite {
        items,
        reference: quote! { <#self_ty>::#name },
    }
}

/// Each item collected across the methods, under the `#[cfg]`s of the method
/// declaring it.
fn conditional<T>(items: &[(Vec<TokenStream2>, T)]) -> impl Iterator<Item = Conditional<'_, T>> {
    items.iter().map(|(cfgs, item)| Conditional { cfgs, item })
}

/// Whether a `#[subscribe_message]` parameter is a `Scoped<T>`, read by its
/// last segment as `Piped<P, T>` and `Valid<T>` are: the expansion then
/// resolves it through `nest_rs_ws`'s own `scoped_parameter`, which returns
/// that crate's `Scoped`, so a type of another crate under that name does not
/// compile.
fn is_scoped(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Scoped")
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

/// Emit the `Gateway` override for `on_connect` / `on_disconnect` delegating
/// to the user method, spanned at `at` so rustc refuses two there.
fn hook_override(hook: &str, method: &ImplItemFn, at: Span) -> syn::Result<TokenStream2> {
    let hook_ident = syn::Ident::new(hook, at);
    let method_name = method.sig.ident.clone();
    if !returns_unit(&method.sig.output) {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            format!(
                "a #[{hook}] hook returns `()`, written out or left out: the gateway calls it \
                 and moves on, so a future it returned would be dropped without ever being \
                 polled and an error would be discarded — write `async fn`, and handle a \
                 failure inside the method"
            ),
        ));
    }

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
        let call = await_if_async(&method.sig, quote!(Self::#method_name(self, __client)));
        quote! { #call; }
    } else {
        let call = await_if_async(&method.sig, quote!(Self::#method_name(self)));
        quote! {
            let _ = __client;
            #call;
        }
    };
    Ok(quote_spanned! {at=>
        async fn #hook_ident(&self, __client: &::nest_rs_ws::WsClient) {
            #body
        }
    })
}
