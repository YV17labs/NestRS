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

/// WS's half of the shared posture grammar. Mandatory per message for the same
/// fail-secure reason it is on a `#[query]` and a `#[tool]`: a handler nobody
/// decided a posture for must not compile, rather than reply with rows no ability
/// ever filtered.
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
/// string literal — or the shared value sentence at what was written, never
/// syn's `expected string literal`, which names neither the attribute nor what
/// it takes.
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

/// Split a `#[subscribe_message]` payload argument into (type to deserialize
/// from the wire, pipe info). `Some((Some(pipe), inner))` for `Piped<P, T>`,
/// `Some((None, inner))` for `Valid<T>`, `None` for a plain payload deserialized
/// as-is.
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
    crate::gateway::WS_PAIR
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
    // The impl half collects; it declares nothing. `#[gateway]` one line up
    // declares the `path` and the `version`, which makes this the likeliest
    // place to reach for either — so an argument list is refused rather than
    // silently dropped.
    if let Err(err) = crate::gateway::WS_PAIR.reject_args(
        &TokenStream2::from(args),
        "a gateway's `path`, `version` and `namespace` are declared by",
    ) {
        return err.to_compile_error().into();
    }
    let mut item = match crate::gateway::WS_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    if let Err(err) = crate::gateway::WS_PAIR.reject_host_layers(&item.attrs) {
        return err.to_compile_error().into();
    }
    let self_ty = item.self_ty.clone();

    // Gateway struct name — logged as a structured field beside each mounted
    // event at boot, mirroring how `#[routes]` logs its controller.
    let host = match impl_self_ident(&self_ty, "#[messages]") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let gateway_name = LitStr::new(&host.to_string(), host.span());

    let mut arms: Vec<TokenStream2> = Vec::new();
    let mut mounted_logs: Vec<TokenStream2> = Vec::new();
    let mut chain_inserts: Vec<TokenStream2> = Vec::new();
    // Folded into `Discoverable::injected` for the access-graph check, same
    // as HTTP per-route layer keys.
    let mut all_message_layers: Vec<(Vec<TokenStream2>, Path)> = Vec::new();
    // Every hook override, each under its method's conditions. All of them are
    // emitted: keeping the last one seen dropped a hook compiled in whenever a
    // compiled-out one followed it, and two compiled in are two definitions of
    // one trait method, which rustc refuses.
    let mut hooks: Vec<TokenStream2> = Vec::new();
    // What each event and hook is already served by. A second declaration is
    // refused at its own attribute — by the macro when neither carries a
    // condition, by rustc through the marker when both are compiled in — instead
    // of compiling to a match arm that never runs while the guard table keeps
    // the *second* method's chain for the first method's arm.
    let mut declared = DispatchKeys::new(
        "#[messages]",
        "a gateway dispatches each event, and each connection hook, to one method, so the \
         second would never run — fold the two bodies into one",
    );

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // One role per method — a message, or one of the two connection hooks —
        // through the family's helper, which places the caret on the repeated
        // attribute rather than serving the first and leaving the rest.
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
        // The hook's own trait method is the marker: two compiled in are two
        // definitions of `on_connect`, spanned at the attributes that declared
        // them. An event has no item of its own to collide, so it gets one.
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
        // The message's access posture, taken off the method so neither attribute
        // reaches the compiler as unknown. `#[public]` is a *declaration* here,
        // not a fast-path: unlike an HTTP route there is no anonymous shortcut to
        // take, so what it buys is the statement that the posture was decided.
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
                // A `Piped<P, T>` / `Valid<T>` payload is a per-argument pipe:
                // deserialize the wire value `T`, run the pipe, then hand the
                // handler the carrier. A rejection replies with `WsReply::error`
                // — the transport analog of the HTTP / GraphQL pipe forms.
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
                                    // Through `pipe_error`, so the frame carries
                                    // the rejection's per-field detail as
                                    // `data.errors` — the same member HTTP
                                    // renders. Formatting only `message()` here
                                    // is what used to drop it.
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
                        // Through `payload_error`, which carries the `warn` on
                        // `nest_rs::ws` a denied dispatch owes an operator —
                        // a client sending garbage used to be the one refusal
                        // that logged nothing at any level.
                        ::core::result::Result::Err(__e) => {
                            return ::nest_rs_ws::WsReply::payload_error(#event, &__e);
                        }
                    };
                    #wrap
                }
            }
            None => quote! {},
        };
        // Step 1 — the class gate, before the payload is deserialized or piped, so
        // a caller the gate refuses never pays for validation and a validation
        // message never doubles as an existence oracle. (The guard chain ran
        // earlier still, in the dispatcher: a gateway is `Guarded`, so its
        // upgrade already carried the real HTTP chain.)
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

        // Step 2 — reply masking, armed by the same posture. The masked *JSON* is
        // what ships, not a value round-tripped back through the handler's type:
        // a WS envelope promises no schema, so a stripped key is simply absent
        // from the frame, exactly as HTTP omits it from a body. See
        // `nest_rs_authz::ws::mask` for why WS is HTTP's case here and not MCP's.
        // `__ret` is the value left once the handler's `Result` and any `Result`
        // inside it are split, bound by the arms below — so this is a value
        // rather than a function of one, and a mask never sees an `Err`.
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

        // A masked message says `Result` outright. Two reasons, and the first is
        // the one a developer would not guess: the reply-shape decision is
        // *syntactic*, so a `Result` behind an alias reads as an ordinary value
        // here and would be masked as the `Result` itself — a frame shaped like a
        // success carrying `{"Ok": …}`. The second is ordinary: a fail-closed mask
        // needs an error channel.
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

        // Every `Err` a handler can return is turned into a frame by type, through
        // `ErrorReport`'s three tiers — the imports bring the two trait tiers into
        // scope, and the inherent one needs none.
        let report = quote! {
            {
                #[allow(unused_imports)]
                use ::nest_rs_ws::{ErrorReportChain as _, ErrorReportFallback as _};
                ::nest_rs_ws::ErrorReport(__err).into_frame(#event)
            }
        };
        // A handler's value is split once more before it replies: a `Result` inside
        // the `Result` is a failure too, however either is spelled. Through
        // `ReplyValue` rather than by reading the type, because an alias hides
        // the `Result` from the macro and not from method resolution.
        let split_then_reply = quote! {
            {
                #[allow(unused_imports)]
                use ::nest_rs_ws::ReplyValueFallback as _;
                match ::nest_rs_ws::ReplyValue(__ret).into_outcome() {
                    ::nest_rs_ws::ReplyOutcome::Value(__ret) => { #reply }
                    ::nest_rs_ws::ReplyOutcome::Failed(__err) => #report,
                }
            }
        };

        let arm_body = match return_kind {
            ReturnKind::Unit => quote! {
                { #call };
                ::nest_rs_ws::WsReply::None
            },
            // Routed through `ReplyValue` rather than `WsReply::reply` so the
            // decision is made on the **type**: a `Result` spelled through an
            // alias (`ServiceResult<T>`) reads as an ordinary value here, and
            // used to serialize its `Err` variant — the whole error struct —
            // into a frame shaped like a success. Method resolution picks the
            // inherent `Result` impl whatever the alias is called; its `Ok` is
            // then split again, as the literal arm's is.
            //
            // The syntactic `Result` arm below is therefore not redundant: on a
            // literal `Result` it goes straight to `ErrorReport`. `ReplyValue`'s
            // `Result` impl carries no bound on `E`, so no error type can fall
            // to the blanket impl and be serialized whole; an `E` that is
            // neither an error nor `Display` fails to compile at `ErrorReport`.
            ReturnKind::Value => quote! {
                let __ret = { #call };
                {
                    #[allow(unused_imports)]
                    use ::nest_rs_ws::ReplyValueFallback as _;
                    match ::nest_rs_ws::ReplyValue(__ret).into_outcome() {
                        ::nest_rs_ws::ReplyOutcome::Value(__ret) => #split_then_reply,
                        ::nest_rs_ws::ReplyOutcome::Failed(__err) => #report,
                    }
                }
            },
            ReturnKind::ResultUnit => quote! {
                match { #call } {
                    ::core::result::Result::Ok(()) => ::nest_rs_ws::WsReply::None,
                    ::core::result::Result::Err(__err) => #report,
                }
            },
            ReturnKind::Result => quote! {
                match { #call } {
                    ::core::result::Result::Ok(__ret) => #split_then_reply,
                    ::core::result::Result::Err(__err) => #report,
                }
            },
        };

        arms.push(quote! { #(#cfgs)* #event => { #arm_body } });
    }

    // Per-message guards run `Guard::check_ws_message`, whose default is `Ok(())`.
    // One bound each, at the `#[use_guards]` line. Guards on the `#[gateway]`
    // struct are deliberately absent from this list: they run on the upgrade,
    // which is an HTTP `GET`.
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
                // Self-mount on HTTP: a WS upgrade is an HTTP `GET`, so a
                // gateway is just another `HttpEndpointMeta` at boot.
                //
                // The *effective* path — `PATH` with `#[gateway(version = …)]`
                // folded in — is what the meta carries, and that matters beyond
                // routing: the transport's duplicate-mount check compares
                // `HttpEndpointMeta::path`, so two gateways declaring one path
                // under two versions are two mounts that both boot, while two on
                // the same path *and* version still fail boot naming both.
                builder.attach_meta::<#self_ty, ::nest_rs_ws::nest_rs_http::HttpEndpointMeta>(
                    ::nest_rs_ws::nest_rs_http::HttpEndpointMeta::new(
                        <#self_ty>::__nestrs_mount_path(),
                        "ws",
                        |__container, __route| {
                            // One string for the log and the mount, so what boot
                            // prints is the address a client connects to.
                            let __path = <#self_ty>::__nestrs_mount_path();
                            #(#mounted_logs)*
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
                            let __data_pipe = ::nest_rs_ws::resolve_ws_data_pipe(__container);
                            let __ep = ::nest_rs_ws::gateway_endpoint(
                                __gw, __server, __chains, __ctx, __data_pipe,
                            );
                            let __ep = <#self_ty>::__nestrs_gateway_layers(__container, __ep);
                            __route.at(__path, __ep)
                        },
                    )
                    .owned_by(#gateway_name)
                    .self_guarded_if(<#self_ty>::HAS_EDGE_GUARDS),
                )
                // Boot-time validation of the **upgrade** chain — the same
                // check `#[routes]` runs for a controller, on the same kind of
                // chain: a `#[gateway(...)]` guard runs on the HTTP `GET` that
                // becomes the socket, so it executes `check_http` and attests
                // `HttpGuard`. A misordered pair fails the boot here instead of
                // denying every connection with nothing to say why.
                //
                // **The per-message chains are deliberately not validated**, and
                // that is a finding rather than an omission. `validate_guard_chain`
                // reads `produced_principal`/`expected_principal`, which describe
                // `check_http`; at the per-message site the entry is
                // `check_ws_message`, where `AuthnGuard` keeps the no-op default
                // by design. Applied there the check was wrong in both
                // directions — silently green on a chain that attaches no
                // principal, and a false boot failure on the split-scope shape
                // `authn-authz.md` sanctions — and it refused `#[force_guards]`
                // under the canonical pool. Answering at that site needs a
                // per-message notion of "producer", which is a design question,
                // not a patch. See `guards-baseline.txt`.
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
/// to the user method. The hook may declare an optional `&WsClient` parameter,
/// and answers `()`.
///
/// The override is named at `at`, the attribute that declared it, so two hooks
/// compiled in are refused by rustc there.
fn hook_override(hook: &str, method: &ImplItemFn, at: Span) -> syn::Result<TokenStream2> {
    let hook_ident = syn::Ident::new(hook, at);
    let method_name = method.sig.ident.clone();
    // The override awaits what the method is and discards what it answers, so a
    // returned future would be dropped without ever being polled, and a returned
    // error with nothing to report it.
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

/// The three inputs one event's chain is composed from, bound as `__global`,
/// `__method`, `__force` and `__label`, reading `__global_guards` and
/// `__container` from the enclosing scope.
fn chain_specs(event: &LitStr, method_guards: &[Path], force_guards: &[Path]) -> TokenStream2 {
    let method_spec_entries = method_guards.iter().map(|p| {
        quote! {
            ::nest_rs_guards::layer_chain::ResolvedLayer {
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
            ::nest_rs_guards::layer_chain::ResolvedLayer<dyn ::nest_rs_guards::Guard>
        > = __global_guards
            .iter()
            .map(|(__tid, __name, __arc)| ::nest_rs_guards::layer_chain::ResolvedLayer {
                type_id: *__tid,
                name: __name,
                source: ::nest_rs_guards::layer_chain::LayerSite::Global,
                layer: ::std::sync::Arc::clone(__arc),
            })
            .collect();
        let __method: ::std::vec::Vec<
            ::nest_rs_guards::layer_chain::ResolvedLayer<dyn ::nest_rs_guards::Guard>
        > = ::std::vec![#(#method_spec_entries),*];
        let __force: ::std::vec::Vec<::core::any::TypeId> = ::std::vec![#(#force_typeids),*];
        let __label = ::std::format!("ws {}", #event);
    }
}

/// Build the chain-insert for one `#[subscribe_message]` event.
///
/// The chain is `global + method_guards`, deduped by `TypeId` (broadest
/// wins). `#[force_guards]` lets a per-message guard replay even when the
/// same `TypeId` is global.
fn chain_insert(event: &LitStr, method_guards: &[Path], force_guards: &[Path]) -> TokenStream2 {
    let specs = chain_specs(event, method_guards, force_guards);
    quote! {
        {
            #specs
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
