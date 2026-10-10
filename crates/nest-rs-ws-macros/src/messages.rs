//! `#[messages]` — bind a `#[gateway]` impl block's `#[subscribe_message]`
//! methods to WebSocket events; emit the `Gateway` dispatcher and the
//! `Discoverable` impl that self-mounts on the HTTP transport. Each event's
//! guard chain is composed once at mount.

use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, ImplItem, ImplItemFn, LitStr, Path, ReturnType, Type};

use nest_rs_codegen::{
    Collision, Conditional, DispatchKeys, HostBorrow, PipeWrapper, Posture, PostureRules,
    await_if_async, cfg_attrs, guard_capability_bounds, impl_self_ident,
    injected_methods_with_layers, layer_deps, pipe_wrapper, reject_http_only_layers, returns_unit,
    shared_receiver, take_flag_attr, take_path_list,
};

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
            nest_rs_codegen::takes_value(
                "subscribe_message",
                None,
                "the event's name as a string literal, e.g. `#[subscribe_message(\"chat\")]`",
            ),
        )
    };
    let written: syn::Expr = attr.parse_args().map_err(|_| refused(attr))?;
    match nest_rs_codegen::ungrouped_expr(&written) {
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
    if let Err(err) = pair::WS.reject_host_layers(&item.attrs) {
        return err.to_compile_error().into();
    }
    let self_ty = item.self_ty.clone();

    let host = match impl_self_ident(&self_ty, "#[messages]") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let gateway_name = LitStr::new(&host.to_string(), host.span());

    let mut arms: Vec<TokenStream2> = Vec::new();
    let mut mounted_logs: Vec<TokenStream2> = Vec::new();
    let mut chain_inserts: Vec<TokenStream2> = Vec::new();
    let mut all_message_layers: Vec<(Vec<TokenStream2>, Path)> = Vec::new();
    // Every hook override is emitted under its method's `#[cfg]`; two compiled in
    // are two definitions of one trait method, which rustc refuses.
    let mut hooks: Vec<TokenStream2> = Vec::new();
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

        let index = match nest_rs_codegen::one_role_per_method(
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
            .and_then(|()| nest_rs_codegen::concrete_signature(method, "#[messages]"))
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

        if let Err(err) = reject_http_only_layers(&method.attrs, "WebSockets", "message") {
            return err.to_compile_error().into();
        }
        let guards = match take_path_list(&mut method.attrs, "use_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let force_guards = match take_path_list(&mut method.attrs, "force_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let posture = match POSTURE.take(method) {
            Ok(posture) => posture,
            Err(err) => return err.to_compile_error().into(),
        };
        all_message_layers.extend(
            guards
                .iter()
                .chain(&force_guards)
                .map(|guard| (cfgs.clone(), guard.clone())),
        );

        let insert = chain_insert(&event, &guards, &force_guards);
        chain_inserts.push(quote! { #(#cfgs)* let () = #insert; });

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
                                    return ::nest_rs_ws::WsReply::pipe_error(
                                        #event, "payload", __e,
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
                            return ::nest_rs_ws::WsReply::payload_error(#event, &__e);
                        }
                    };
                    #wrap
                }
            }
            None => quote! {},
        };
        // The class gate runs before the payload is deserialized, so a validation
        // message never doubles as an existence oracle.
        let gate = match &posture {
            Posture::Authorize { action, entity, .. } => quote! {
                if let ::core::result::Result::Err(__denied) =
                    ::nest_rs_authz::ws::authorize::<#action, #entity>(#event)
                {
                    return ::nest_rs_ws::WsReply::Error(__denied);
                }
            },
            Posture::Public => quote! {},
        };

        let invoke = await_if_async(
            &method.sig,
            quote!(<#self_ty>::#method_name(self, #(#call_args),*)),
        );
        let call = quote! {
            #gate
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
                        ::nest_rs_ws::WsReply::Reply(__masked)
                    }
                    ::core::result::Result::Err(__failed) => {
                        ::nest_rs_ws::WsReply::Error(__failed)
                    }
                }
            },
            _ => quote! { ::nest_rs_ws::WsReply::reply(&__ret) },
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
        // `Infallible` error that call would be unreachable code.
        let report = quote! {
            {
                #[allow(unused_imports)]
                use ::nest_rs_ws::{ErrorReportChain as _, ErrorReportFallback as _};
                __report.into_frame(#event)
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
                ::nest_rs_ws::WsReply::None
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
                match ::core::result::Result::map_err({ #call }, ::nest_rs_ws::ErrorReport) {
                    ::core::result::Result::Ok(()) => ::nest_rs_ws::WsReply::None,
                    ::core::result::Result::Err(__report) => #report,
                }
            },
            ReturnKind::Result => quote! {
                match ::core::result::Result::map_err({ #call }, ::nest_rs_ws::ErrorReport) {
                    ::core::result::Result::Ok(__ret) => #split_then_reply,
                    ::core::result::Result::Err(__report) => #report,
                }
            },
        };

        arms.push(quote! { #(#cfgs)* #event => { #arm_body } });
    }

    // `Guard::check_ws_message` defaults to `Ok(())`, so each per-message guard
    // must attest `WsGuard`. Gateway-scope guards run on the HTTP upgrade instead.
    let conditional = || {
        all_message_layers
            .iter()
            .map(|(cfgs, item)| Conditional { cfgs, item })
    };
    let capability_bounds =
        guard_capability_bounds(conditional(), quote!(::nest_rs_guards::WsGuard));
    let message_layers = layer_deps(conditional());
    let injected_methods = injected_methods_with_layers(&self_ty, &message_layers);
    let markers = declared.markers(&self_ty, &item.generics);

    quote! {
        #item

        #capability_bounds

        #markers

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
                // Two arms for one event are refused — by the macro, or by rustc
                // at the marker — so the lint would only repeat that refusal.
                #[allow(unreachable_patterns)]
                match __event {
                    #(#arms)*
                    __other => ::nest_rs_ws::WsReply::unknown(__other),
                }
            }

            #(#hooks)*
        }

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_methods

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                // poem stops tracking a socket at the upgrade: the `DetachedWork` lets
                // the transport tell each socket at shutdown and bound its window.
                let __sockets = ::nest_rs_ws::nest_rs_http::DetachedWork::new();
                let __carried = __sockets.clone();
                builder.attach_meta::<#self_ty, ::nest_rs_ws::nest_rs_http::HttpEndpointMeta>(
                    ::nest_rs_ws::nest_rs_http::HttpEndpointMeta::new(
                        <#self_ty>::__nestrs_mount_path(),
                        "ws",
                        move |__container, __route| {
                            let __path = <#self_ty>::__nestrs_mount_path();
                            #(#mounted_logs)*
                            let __gw = ::std::sync::Arc::new(
                                <#self_ty>::from_container(__container),
                            );
                            let __server = <#self_ty>::__nestrs_registry(__container);
                            let mut __chains = ::nest_rs_ws::EventLayerTable::new();
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
                            let __data_pipe = ::nest_rs_ws::__private::resolve_ws_data_pipe(__container);
                            let __ep = ::nest_rs_ws::gateway_endpoint(
                                __gw,
                                __server,
                                __chains,
                                __ctx,
                                __data_pipe,
                                ::core::clone::Clone::clone(&__carried),
                            );
                            let __ep = <#self_ty>::__nestrs_gateway_layers(__container, __ep);
                            __route.at(__path, ::nest_rs_ws::nest_rs_http::matched(__ep))
                        },
                    )
                    .owned_by(#gateway_name)
                    .self_guarded_if(<#self_ty>::HAS_EDGE_GUARDS)
                    .runs_detached(__sockets),
                )
                // Only the upgrade chain is validated: the principal check describes
                // `check_http`, and per-message chains have no notion of a producer yet.
                .attach_meta::<#self_ty, ::nest_rs_ws::nest_rs_http::HttpBootCheck>(
                    ::nest_rs_ws::nest_rs_http::HttpBootCheck::new(|__container| {
                        ::nest_rs_guards::dispatch::boot_validate_guards(
                            __container,
                            &<#self_ty>::__nestrs_edge_guard_specs(),
                            &::std::format!(
                                "the {} gateway upgrade",
                                <#self_ty>::__nestrs_mount_path(),
                            ),
                        )
                    }),
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

/// One event's chain inputs, bound as `__global`, `__method`, `__force` and
/// `__label`, reading `__global_guards` and `__container` from the enclosing scope.
fn chain_specs(event: &LitStr, method_guards: &[Path], force_guards: &[Path]) -> TokenStream2 {
    let method_spec_entries = method_guards.iter().map(|p| {
        quote! {
            ::nest_rs_guards::__private::ResolvedLayer {
                type_id: ::core::any::TypeId::of::<#p>(),
                name: ::core::any::type_name::<#p>(),
                source: ::nest_rs_guards::layer_chain::LayerSite::Method,
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
        let __global: ::std::vec::Vec<
            ::nest_rs_guards::__private::ResolvedLayer<dyn ::nest_rs_guards::Guard>
        > = __global_guards
            .iter()
            .map(|(__tid, __name, __arc)| ::nest_rs_guards::__private::ResolvedLayer {
                type_id: *__tid,
                name: __name,
                source: ::nest_rs_guards::layer_chain::LayerSite::Global,
                layer: ::std::sync::Arc::clone(__arc),
            })
            .collect();
        let __method: ::std::vec::Vec<
            ::nest_rs_guards::__private::ResolvedLayer<dyn ::nest_rs_guards::Guard>
        > = ::std::vec![#(#method_spec_entries),*];
        let __force: ::std::vec::Vec<::core::any::TypeId> = ::std::vec![#(#force_typeids),*];
        let __label = ::std::format!("ws {}", #event);
    }
}

/// Build the chain-insert for one `#[subscribe_message]` event: global plus
/// method guards, deduped by `TypeId` unless `#[force_guards]` replays one.
fn chain_insert(event: &LitStr, method_guards: &[Path], force_guards: &[Path]) -> TokenStream2 {
    let specs = chain_specs(event, method_guards, force_guards);
    quote! {
        {
            #specs
            let __chain = ::nest_rs_guards::__private::compose_chain::<dyn ::nest_rs_guards::Guard>(
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
                    let __wrapped = ::nest_rs_guards::GuardAsWsMessageCheck::new(
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
