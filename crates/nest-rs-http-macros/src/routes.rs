//! `#[routes]` — bind a `#[controller]` impl block's verb-tagged methods to
//! HTTP routes; emit `Controller` mount + `Discoverable`; capture per-route
//! OpenAPI metadata.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, ImplItem, ItemImpl, LitStr, Meta, Path, ReturnType, Token, Type,
    parse_macro_input,
};

use nest_rs_codegen::{
    expr_str, force_guard_typeids, forwarded_arg_idents, impl_self_ident,
    injected_method_with_layers, layer_inject_keys, nth_generic_type, scoped_specs, take_flag_attr,
    take_path_list,
};

use crate::attr::opt_str;

/// One route handler, by named field — a positional tuple here once let a
/// field-order slip silently swap e.g. `force_guards`/`pipes`.
struct RouteHandler {
    /// The HTTP verb ident (`get`, `post`, …).
    verb: syn::Ident,
    /// The generated wrapper fn's ident.
    wrapper: syn::Ident,
    /// `#[use_guards]` paths on the method.
    guards: Vec<Path>,
    /// `#[use_filters]` paths on the method.
    filters: Vec<Path>,
    /// `#[use_interceptors]` paths on the method.
    interceptors: Vec<Path>,
    /// The `Authorize<_, _>` / `Bind<_, _>` shaper type, if any.
    shaper: Option<Type>,
    /// `#[meta(...)]` value expressions.
    metas: Vec<Expr>,
    /// The `#[public]` flag.
    is_public: bool,
    /// The `#[no_pipes]` opt-out flag.
    no_pipes: bool,
    /// `#[force_guards]` paths on the method.
    force_guards: Vec<Path>,
    /// `#[use_pipes]` paths on the method.
    pipes: Vec<Path>,
    /// `#[use_exception_filters]` paths on the method.
    exception_filters: Vec<Path>,
}

/// Handlers grouped by path in first-seen order. Several verbs may share a
/// path (`GET` + `POST /users`), and poem rejects two `.at(path, ..)` for the
/// same path, so they must collapse into one `RouteMethod` (`get(h1).post(h2)`).
type RoutesByPath = Vec<(LitStr, Vec<RouteHandler>)>;

pub(crate) fn routes(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    // Default OpenAPI tag — routes group by controller unless `#[api(tags(...))]` overrides.
    let ctrl_name = match impl_self_ident(&self_ty, "routes") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let ctrl_tag = LitStr::new(&ctrl_name.to_string(), ctrl_name.span());

    let mut wrappers: Vec<TokenStream2> = Vec::new();
    let mut routes_by_path: RoutesByPath = Vec::new();
    let mut route_metas: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let verb_idx = method.attrs.iter().position(|attr| {
            ["get", "post", "put", "delete", "patch"]
                .iter()
                .any(|v| attr.path().is_ident(v))
        });
        let Some(idx) = verb_idx else { continue };

        let attr = method.attrs.remove(idx);
        let verb_ident = attr
            .path()
            .get_ident()
            .expect("verb attribute has an ident")
            .clone();

        let route_path: LitStr = match attr.parse_args() {
            Ok(p) => p,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_name = method.sig.ident.clone();
        let method_name_lit = method_name.to_string();
        let wrapper_name = format_ident!("__nestrs_route_{}", method_name);

        let mut inputs: Vec<FnArg> = method.sig.inputs.iter().skip(1).cloned().collect();
        // `#[authorize(Action, Entity)]` — the route's posture, uniform with
        // `#[resolver]`. Desugars to the shaper extractor the macro writes
        // itself, so the arming can no longer be broken by how the developer
        // spelled (or renamed) an import.
        let authorize = match take_authorize(&mut method.attrs) {
            Ok(spec) => spec,
            Err(err) => return err.to_compile_error().into(),
        };
        if let Some(spec) = &authorize {
            match authorize_param(spec, &inputs) {
                Ok(param) => inputs.insert(0, param),
                Err(err) => return err.to_compile_error().into(),
            }
        }
        let arg_idents = match forwarded_arg_idents(&method.sig) {
            Ok(idents) => idents,
            Err(err) => return err.to_compile_error().into(),
        };

        let return_type = match &method.sig.output {
            ReturnType::Default => quote! { () },
            ReturnType::Type(_, ty) => quote! { #ty },
        };

        let extra_inputs = if inputs.is_empty() {
            quote! {}
        } else {
            quote! { , #(#inputs),* }
        };

        let guards = match take_path_list(&mut method.attrs, "use_guards", "entry") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        // Captured before `guards` is moved into the handler tuple — feeds the
        // route's `scoped_guarded` flag (combined at runtime with any
        // controller-level guards) for the boot-time posture check.
        let method_guarded = !guards.is_empty();
        // Likewise for the `throttled` flag (OAPI-O4): a method-level
        // `ThrottlerGuard` here, or a controller-level one via the runtime call
        // emitted below.
        let method_throttled = guards.iter().any(guard_path_is_throttler);
        let force_guards = match take_path_list(&mut method.attrs, "force_guards", "entry") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let filters = match take_path_list(&mut method.attrs, "use_filters", "entry") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let interceptors = match take_path_list(&mut method.attrs, "use_interceptors", "entry") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let method_pipes = match take_path_list(&mut method.attrs, "use_pipes", "entry") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let method_exception_filters =
            match take_path_list(&mut method.attrs, "use_exception_filters", "entry") {
                Ok(paths) => paths,
                Err(err) => return err.to_compile_error().into(),
            };
        // `#[public]` marks the route as publicly reachable; global guards still
        // run and decide whether to admit anonymous callers.
        let is_public = take_flag_attr(&mut method.attrs, "public");
        if is_public && let Some(spec) = &authorize {
            return syn::Error::new_spanned(
                &spec.action,
                "a route is `#[public]` or `#[authorize(...)]`, never both — the two \
                 declare opposite postures",
            )
            .to_compile_error()
            .into();
        }
        // `#[no_pipes]` opts out of every global pipe for this route.
        let no_pipes = take_flag_attr(&mut method.attrs, "no_pipes");
        // Internal marker the `#[crud]` macro stamps on its write ops (create /
        // update / delete) — their write-error mapper can surface a `409` on a
        // uniqueness violation, so the document advertises that response. Always
        // stripped here so it never reaches the compiler.
        let may_conflict = take_flag_attr(&mut method.attrs, "crud_write");

        // Drained after the `use_*` attributes so error spans for a misuse of
        // a response decorator point past the layers — and *before* emitting
        // the wrapper fn so the wrapper's return type and body reflect any
        // status / header / redirect override. The method's block is forwarded
        // so `#[redirect]` can reject a non-empty body (which the macro would
        // silently drop).
        let response_shapers =
            match crate::response::take_response_shapers(&mut method.attrs, &method.block) {
                Ok(d) => d,
                Err(err) => return err.to_compile_error().into(),
            };
        // The effective success status the OpenAPI document advertises for this
        // route (OAPI-O3): a `#[redirect]`/`#[http_code(N)]` overrides the 200
        // default.
        let success_status = response_shapers.success_status();

        let call_expr = quote! { __ctrl.#method_name(#(#arg_idents),*).await };
        let returns_result = match &method.sig.output {
            ReturnType::Type(_, ty) => result_inner(ty).is_some(),
            ReturnType::Default => false,
        };
        let (wrapper_return_type, wrapper_body) = if response_shapers.is_empty() {
            (return_type.clone(), call_expr)
        } else {
            let mut wrapper_args: Vec<syn::Ident> = Vec::with_capacity(arg_idents.len() + 1);
            wrapper_args.push(syn::Ident::new("__ctrl", proc_macro2::Span::call_site()));
            wrapper_args.extend(arg_idents.iter().cloned());
            let body = crate::response::apply_response_shapers(
                &response_shapers,
                call_expr,
                &wrapper_args,
                returns_result,
            );
            (
                quote! { ::nest_rs_http::poem::Result<::nest_rs_http::poem::Response> },
                body,
            )
        };

        wrappers.push(quote! {
            #[::nest_rs_http::poem::handler]
            async fn #wrapper_name(
                ::nest_rs_http::poem::web::Data(__ctrl): ::nest_rs_http::poem::web::Data<&::std::sync::Arc<#self_ty>>
                #extra_inputs
            ) -> #wrapper_return_type {
                #wrapper_body
            }
        });

        let mut metas: Vec<Expr> = Vec::new();
        while let Some(m_idx) = method.attrs.iter().position(|a| a.path().is_ident("meta")) {
            let m_attr = method.attrs.remove(m_idx);
            match m_attr.parse_args::<Expr>() {
                Ok(expr) => metas.push(expr),
                Err(err) => return err.to_compile_error().into(),
            }
        }

        // Detected by name so this crate stays free of any dep on the authz crate.
        let shaper = shaper_type(&inputs);

        let handler = RouteHandler {
            verb: verb_ident.clone(),
            wrapper: wrapper_name.clone(),
            guards,
            filters,
            interceptors,
            shaper: shaper.clone(),
            metas,
            is_public,
            no_pipes,
            force_guards,
            pipes: method_pipes,
            exception_filters: method_exception_filters,
        };
        match routes_by_path
            .iter_mut()
            .find(|(path, _)| path.value() == route_path.value())
        {
            Some((_, handlers)) => {
                // Two handlers for the same (verb, path) would collapse silently
                // into `poem::get(h1).get(h2)` — the second wins and the first
                // becomes dead, unroutable code. Reject it at the macro (HTTP-R2).
                if handlers.iter().any(|h| h.verb == verb_ident) {
                    return syn::Error::new_spanned(
                        &verb_ident,
                        format!(
                            "duplicate route `{} {}` on this controller — two handlers for \
                             the same verb+path collapse silently (the later one would win); \
                             give one a distinct path or verb",
                            verb_ident.to_string().to_uppercase(),
                            route_path.value(),
                        ),
                    )
                    .to_compile_error()
                    .into();
                }
                handlers.push(handler);
            }
            None => routes_by_path.push((route_path.clone(), vec![handler])),
        }

        let verb_variant = match verb_ident.to_string().as_str() {
            "get" => quote!(::nest_rs_http::HttpVerb::Get),
            "post" => quote!(::nest_rs_http::HttpVerb::Post),
            "put" => quote!(::nest_rs_http::HttpVerb::Put),
            "delete" => quote!(::nest_rs_http::HttpVerb::Delete),
            "patch" => quote!(::nest_rs_http::HttpVerb::Patch),
            _ => unreachable!("verb_ident filtered above"),
        };

        let api = match method.attrs.iter().position(|a| a.path().is_ident("api")) {
            Some(a_idx) => {
                let a_attr = method.attrs.remove(a_idx);
                match parse_api_attr(&a_attr) {
                    Ok(api) => api,
                    Err(err) => return err.to_compile_error().into(),
                }
            }
            None => ApiMeta::default(),
        };
        let summary = opt_str(&api.summary);
        let description = opt_str(&api.description);
        let tags = if api.tags.is_empty() {
            quote! { &[#ctrl_tag] }
        } else {
            let tags = &api.tags;
            quote! { &[#(#tags),*] }
        };

        let request_body = match request_payload(&inputs) {
            Some(ty) => quote! {
                ::core::option::Option::Some(::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn)
            },
            None => quote! { ::core::option::Option::None },
        };
        // A shaped (masked) response has no static schema — the fields it
        // carries depend on the caller's ability — so skip schema capture there.
        let response = match (shaper.is_some(), response_payload(&method.sig.output)) {
            (false, Some(ty)) => quote! {
                ::core::option::Option::Some(::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn)
            },
            _ => quote! { ::core::option::Option::None },
        };

        // `Path<T>` extractor types (in path order) and `Query<T>` payload
        // types — the OpenAPI doc turns the former into real path-param schemas
        // (`Uuid` → `format: uuid`, `i64` → `integer`) and expands each of the
        // latter's object properties into individual query parameters. Both
        // impose `JsonSchema` on the captured type, as `Json<T>` bodies do.
        let path_param_tys = path_param_types(&inputs);
        let path_params = if path_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#path_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };
        let query_param_tys = query_payloads(&inputs);
        let query_params = if query_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#query_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };

        route_metas.push(quote! {
            ::nest_rs_http::HttpRouteMeta {
                verb: #verb_variant,
                path: #route_path,
                handler: #method_name_lit,
                summary: #summary,
                description: #description,
                tags: #tags,
                request_body: #request_body,
                response: #response,
                path_params: #path_params,
                query_params: #query_params,
                may_conflict: #may_conflict,
                throttled: #method_throttled
                    || <#self_ty>::__nestrs_controller_has_throttler(),
                success_status: #success_status,
                scoped_guarded: #method_guarded
                    || !<#self_ty>::__nestrs_controller_guard_specs().is_empty(),
                public: #is_public,
            }
        });
    }

    // Per-route layers fold into the access-graph dependencies so an unimported
    // module fails boot with an `AccessGraphError`, not a silent resolution.
    let route_layer_keys = layer_inject_keys(
        routes_by_path
            .iter()
            .flat_map(|(_, handlers)| handlers.iter())
            .flat_map(|handler| {
                handler
                    .guards
                    .iter()
                    .chain(&handler.filters)
                    .chain(&handler.interceptors)
                    .chain(&handler.force_guards)
                    .chain(&handler.pipes)
                    .chain(&handler.exception_filters)
            }),
    );
    let injected_method = injected_method_with_layers(&self_ty, &route_layer_keys);

    let route_entries: Vec<TokenStream2> = routes_by_path
        .iter()
        .map(|(path, handlers)| {
            let mut iter = handlers.iter();
            let first = iter.next().expect("each path has at least one verb");
            let first_label = format!("{} {}", first.verb, path.value());
            let first_ep = guarded_handler(first, &first_label, &self_ty);
            let first_verb = &first.verb;
            let mut method = quote! { ::nest_rs_http::poem::#first_verb(#first_ep) };
            for handler in iter {
                let label = format!("{} {}", handler.verb, path.value());
                let ep = guarded_handler(handler, &label, &self_ty);
                let verb = &handler.verb;
                method = quote! { #method.#verb(#ep) };
            }
            quote! { .at(#path, #method) }
        })
        .collect();

    quote! {
        #item

        #(#wrappers)*

        impl ::nest_rs_http::Controller for #self_ty {
            fn mount(
                container: &::nest_rs_core::Container,
                route: ::nest_rs_http::poem::Route,
            ) -> ::nest_rs_http::poem::Route {
                use ::nest_rs_http::poem::EndpointExt;
                let __ctrl = ::std::sync::Arc::new(<#self_ty>::from_container(container));
                let __sub = ::nest_rs_http::poem::Route::new()
                    #(#route_entries)*
                    .data(__ctrl);
                let __prefix = ::nest_rs_http::version_path(<#self_ty>::VERSION, <#self_ty>::PATH);
                route.nest(__prefix.as_str(), __sub)
            }
        }

        impl ::nest_rs_core::Discoverable for #self_ty {
            // `dependencies` stays empty (controller is built at mount); `injected`
            // reports `#[inject]` keys + every container-resolved layer for the
            // access-graph check.
            #injected_method

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                let __meta = ::nest_rs_http::HttpControllerMeta::new(
                    #ctrl_tag,
                    <#self_ty>::PATH,
                    <#self_ty>::VERSION,
                    ::std::vec![#(#route_metas),*],
                    |__c, __r| <#self_ty as ::nest_rs_http::Controller>::mount(__c, __r),
                );
                builder
                    .attach_meta::<#self_ty, ::nest_rs_http::HttpControllerMeta>(__meta)
                    // Boot-time guard-chain validation for this controller:
                    // declared phase ordering plus the produced/expected
                    // principal cross-check (authn's claims vs. authz's actor)
                    // fail boot with a named error instead of a per-request 500.
                    .attach_meta::<#self_ty, ::nest_rs_http::HttpBootCheck>(
                        ::nest_rs_http::HttpBootCheck::new(|__container| {
                            ::nest_rs_guards::dispatch::boot_validate_guards(
                                __container,
                                &<#self_ty>::__nestrs_controller_guard_specs(),
                                #ctrl_tag,
                            )
                        }),
                    )
            }
        }
    }
    .into()
}

/// A route's `#[authorize(Action, Entity)]` posture.
struct AuthorizeSpec {
    action: Path,
    entity: Path,
}

/// Take `#[authorize(Action, Entity)]` off a route method.
///
/// The HTTP twin of `#[resolver]`'s per-operation posture: one attribute,
/// greppable, and — unlike a parameter the developer spells — impossible to
/// disarm by renaming an import, because the macro writes the extractor type.
fn take_authorize(attrs: &mut Vec<Attribute>) -> syn::Result<Option<AuthorizeSpec>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("authorize")) else {
        return Ok(None);
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident("authorize")) {
        return Err(syn::Error::new_spanned(
            &attr,
            "at most one `#[authorize(...)]` per route",
        ));
    }
    let paths: Vec<Path> = attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect();
    let [action, entity] = <[Path; 2]>::try_from(paths).map_err(|_| {
        syn::Error::new_spanned(
            &attr,
            "expected `#[authorize(Action, Entity)]` — e.g. \
             `#[authorize(Read, users::Entity)]`. Bind the subject with a \
             `Bind<Service, Action>` parameter when the route loads one",
        )
    })?;
    Ok(Some(AuthorizeSpec { action, entity }))
}

/// The extractor `#[authorize(Action, Entity)]` desugars to — the same
/// parameter `#[crud]` emits on its generated ops, so both paths arm the class
/// gate, the response mask and the `MaskProbe` through one mechanism.
fn authorize_param(spec: &AuthorizeSpec, inputs: &[FnArg]) -> syn::Result<FnArg> {
    if let Some(param) = inputs.iter().find(|arg| is_authorize_param(arg)) {
        return Err(syn::Error::new_spanned(
            param,
            "this route already declares its posture with `#[authorize(...)]` — \
             drop the `Authorize<...>` parameter (the decorator emits it)",
        ));
    }
    let AuthorizeSpec { action, entity } = spec;
    Ok(syn::parse_quote! {
        __nestrs_authz: ::nest_rs_authz::http::Authorize<#action, #entity>
    })
}

/// Whether a parameter is a hand-written `Authorize<..>` (not a `Bind<..>`,
/// which stays a legitimate parameter — it loads the subject, it is not the
/// posture).
fn is_authorize_param(arg: &FnArg) -> bool {
    let FnArg::Typed(pt) = arg else { return false };
    let Type::Path(tp) = pt.ty.as_ref() else {
        return false;
    };
    tp.path.segments.iter().any(|s| s.ident == "Authorize")
}

/// Response shaper type: `Authorize<A, S>` or `Bind<S, A>` anywhere in the
/// handler parameter list, matched by path-segment **name** (any module
/// qualification works; a *renamed* import does not). `#[authorize(...)]` is
/// immune — the macro inserts that parameter itself, fully qualified, at
/// position 0. The residual blind spot (a hand-written, renamed `Bind`) is
/// closed at run time: unarmed routes carry `nest_rs_http::mask_probed`, which
/// fails the request closed when a masking extractor runs without an armed
/// shaper.
fn shaper_type(inputs: &[FnArg]) -> Option<Type> {
    inputs.iter().find_map(|arg| {
        let FnArg::Typed(pt) = arg else { return None };
        let Type::Path(tp) = pt.ty.as_ref() else {
            return None;
        };
        shaper_param_type(tp).then_some((*pt.ty).clone())
    })
}

fn shaper_param_type(tp: &syn::TypePath) -> bool {
    let angled = tp
        .path
        .segments
        .last()
        .is_some_and(|s| matches!(s.arguments, syn::PathArguments::AngleBracketed(_)));
    if !angled {
        return false;
    }
    tp.path
        .segments
        .iter()
        .any(|s| s.ident == "Authorize" || s.ident == "Bind")
}

/// Build one routed handler. Layout, inner → outer:
///
/// shaper (mask) → exception-filter pool (all scopes — typed catches sit
/// closest to the handler) → per-route filters (controller + method) →
/// per-route interceptors (controller + method) → `RouteShaper` (guard +
/// pipe pools) → metadata data.
///
/// Every family composes through the same `compose_chain` dedup (global +
/// controller + method, broadest scope wins). Global filters / interceptors
/// participate in the dedup but *execute at the transport edge* (their wrap
/// covers 404s, self-mounts and guard denials); the route site executes the
/// controller / method survivors only, inside the guard chain — a denial
/// short-circuits before any handler-side layer. The relative nesting is the
/// same at both sites: interceptors outside filters, filters outside
/// exception-filters.
fn guarded_handler(handler: &RouteHandler, route_label: &str, self_ty: &Type) -> TokenStream2 {
    let RouteHandler {
        verb: _,
        wrapper,
        guards,
        filters,
        interceptors,
        shaper,
        metas,
        is_public,
        no_pipes,
        force_guards,
        pipes: method_pipes,
        exception_filters: method_exception_filters,
    } = handler;
    let route_label_lit = LitStr::new(route_label, proc_macro2::Span::call_site());
    // An unarmed route carries the run-time mask probe: if a masking extractor
    // (`Authorize`/`Bind` under a rename the name scan cannot see) runs anyway,
    // the request fails closed instead of shipping an unmasked body.
    let mut expr = match shaper {
        Some(ty) => quote! {
            {
                // HTTP-D1: eagerly assert the armed type is a real response
                // shaper, so a false-positive arm (a parameter whose type is
                // *named* `Authorize`/`Bind` but does not implement the shaper
                // trait) is a spanned compile error here — not a confusing
                // transitive `Endpoint` bound failure when the route mounts, and
                // not only the run-time `MaskProbe` net.
                const _: fn() = || {
                    fn __nestrs_assert_route_shaper<P: ::nest_rs_http::RouteResponseShaper>() {}
                    __nestrs_assert_route_shaper::<#ty>();
                };
                ::nest_rs_http::shaped(#wrapper, ::core::marker::PhantomData::<#ty>)
            }
        },
        None => quote! { ::nest_rs_http::mask_probed(#wrapper, #route_label_lit) },
    };
    let method_exception_filter_specs = scoped_specs(
        method_exception_filters,
        quote!(dyn ::nest_rs_exception_filters::ExceptionFilterErased),
    );
    expr = quote! {
        ::nest_rs_guards::dispatch::wrap_route_exception_filters(
            container,
            ::nest_rs_http::poem::EndpointExt::boxed(::nest_rs_http::poem::EndpointExt::map_to_response(#expr)),
            &<#self_ty>::__nestrs_controller_exception_filter_specs(),
            &#method_exception_filter_specs,
            #route_label_lit,
        )
    };
    let method_filter_specs = scoped_specs(filters, quote!(dyn ::nest_rs_filters::Filter));
    expr = quote! {
        ::nest_rs_guards::dispatch::wrap_route_filters(
            container,
            #expr,
            &<#self_ty>::__nestrs_controller_filter_specs(),
            &#method_filter_specs,
            #route_label_lit,
        )
    };
    let method_interceptor_specs = scoped_specs(
        interceptors,
        quote!(dyn ::nest_rs_interceptors::Interceptor),
    );
    expr = quote! {
        ::nest_rs_guards::dispatch::wrap_route_interceptors(
            container,
            #expr,
            &<#self_ty>::__nestrs_controller_interceptor_specs(),
            &#method_interceptor_specs,
            #route_label_lit,
        )
    };

    // RouteShaper sits *inside* the metadata wrap so per-route
    // guards reading `#[meta(...)]` via `Reflector` see it; outside the
    // per-route layer wraps so a denial short-circuits before any
    // handler-side work.
    let method_guard_specs = scoped_specs(guards, quote!(dyn ::nest_rs_guards::Guard));
    let force_guard_typeids = force_guard_typeids(force_guards);
    let method_pipe_specs = scoped_specs(method_pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let no_pipes_flag = if *no_pipes {
        quote!(true)
    } else {
        quote!(false)
    };
    expr = quote! {
        ::nest_rs_interceptors::InterceptorExt::interceptor(
            #expr,
            ::std::sync::Arc::new(
                ::nest_rs_guards::RouteShaper::new(
                    container,
                    #route_label_lit,
                    <#self_ty>::__nestrs_controller_guard_specs(),
                    #method_guard_specs,
                    #force_guard_typeids,
                    <#self_ty>::__nestrs_controller_pipe_specs(),
                    #method_pipe_specs,
                    #no_pipes_flag,
                )
            ),
        )
    };

    // Metadata is attached *after* the RouteShaper so per-route
    // guards see the route's `#[meta]` value when the chain runs.
    for m in metas {
        expr = quote! { ::nest_rs_http::poem::EndpointExt::data(#expr, #m) };
    }

    // `#[public]` attaches a `Public` marker as route data. The framework
    // does not act on it; guards read it via `Reflector::is_public()` and
    // adjust their own policy.
    if *is_public {
        expr = quote! {
            ::nest_rs_http::poem::EndpointExt::data(#expr, ::nest_rs_core::Public)
        };
    }

    expr
}

/// Whether a guard path names the framework's `ThrottlerGuard` — the signal
/// that a route is rate-limited and can answer `429`. Matched on the last path
/// segment's ident so `nest_rs_throttler::ThrottlerGuard`, a `use`-imported
/// `ThrottlerGuard`, and an aliased re-export all count. Name-based by design:
/// the same lightweight detection the masking-arm check uses — a user guard
/// *named* `ThrottlerGuard` that isn't the framework's is a
/// pathological false-positive we accept over dragging a type dependency into
/// the macro crate.
pub(crate) fn guard_path_is_throttler(path: &Path) -> bool {
    path.segments
        .last()
        .is_some_and(|seg| seg.ident == "ThrottlerGuard")
}

#[derive(Default)]
struct ApiMeta {
    summary: Option<LitStr>,
    description: Option<LitStr>,
    tags: Vec<LitStr>,
}

fn parse_api_attr(attr: &Attribute) -> syn::Result<ApiMeta> {
    let mut out = ApiMeta::default();
    let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("summary") => {
                out.summary = Some(expr_str(&nv.value)?);
            }
            Meta::NameValue(nv) if nv.path.is_ident("description") => {
                out.description = Some(expr_str(&nv.value)?);
            }
            Meta::List(list) if list.path.is_ident("tags") => {
                out.tags = list
                    .parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?
                    .into_iter()
                    .collect();
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[api] accepts `summary = \"...\"`, `description = \"...\"`, and \
                     `tags(\"a\", \"b\")`",
                ));
            }
        }
    }
    Ok(out)
}

/// The JSON payload type behind an extractor: `Json<T>`, `Valid<Json<T>>`, and
/// `Piped<_, Json<T>>` all yield `T`. Non-JSON yields `None`.
fn json_payload(ty: &Type) -> Option<Type> {
    if let Some(t) = nth_generic_type(ty, "Json", 0) {
        return Some(t.clone());
    }
    if let Some(inner) = nth_generic_type(ty, "Valid", 0) {
        return json_payload(inner);
    }
    if let Some(inner) = nth_generic_type(ty, "Piped", 1) {
        return json_payload(inner);
    }
    None
}

fn request_payload(inputs: &[FnArg]) -> Option<Type> {
    inputs.iter().find_map(|arg| match arg {
        FnArg::Typed(pt) => json_payload(&pt.ty),
        _ => None,
    })
}

/// The type inside a `Path<T>` extractor: `Path<T>`, `Valid<Path<T>>`, and
/// `Piped<_, Path<T>>` all yield `T`. Non-`Path` yields `None`.
fn path_payload(ty: &Type) -> Option<Type> {
    if let Some(t) = nth_generic_type(ty, "Path", 0) {
        return Some(t.clone());
    }
    if let Some(inner) = nth_generic_type(ty, "Valid", 0) {
        return path_payload(inner);
    }
    if let Some(inner) = nth_generic_type(ty, "Piped", 1) {
        return path_payload(inner);
    }
    None
}

/// The path-parameter types a handler binds, in path order. A single
/// `Path<T>` yields `[T]`; a tuple `Path<(A, B)>` yields `[A, B]` (poem binds
/// tuple elements to the `:name` segments left-to-right). A handler with no
/// `Path<…>` extractor (it binds its id via `Bind<_, _>` instead) yields an
/// empty vec — the doc then guesses `format: uuid` for id-like segments.
fn path_param_types(inputs: &[FnArg]) -> Vec<Type> {
    for arg in inputs {
        let FnArg::Typed(pt) = arg else { continue };
        if let Some(inner) = path_payload(&pt.ty) {
            return match inner {
                Type::Tuple(tuple) => tuple.elems.into_iter().collect(),
                other => vec![other],
            };
        }
    }
    Vec::new()
}

/// The type inside a `Query<T>` extractor: `Query<T>`, `Valid<Query<T>>`, and
/// `Piped<_, Query<T>>` all yield `T`. Non-`Query` yields `None`.
fn query_payload(ty: &Type) -> Option<Type> {
    if let Some(t) = nth_generic_type(ty, "Query", 0) {
        return Some(t.clone());
    }
    if let Some(inner) = nth_generic_type(ty, "Valid", 0) {
        return query_payload(inner);
    }
    if let Some(inner) = nth_generic_type(ty, "Piped", 1) {
        return query_payload(inner);
    }
    None
}

/// Every `Query<T>` payload type in the handler signature, in argument order.
fn query_payloads(inputs: &[FnArg]) -> Vec<Type> {
    inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pt) => query_payload(&pt.ty),
            _ => None,
        })
        .collect()
}

/// `Some(T)` when `ty` is `Result<T, _>`, `None` otherwise. Detects the
/// unqualified last-segment ident `Result` — it does not resolve type
/// aliases (proc-macros have no name resolution), so a feature-local
/// alias whose last segment is `Result` is matched while a renamed
/// `type Outcome<T, E> = Result<T, E>;` is not. That limitation is
/// acceptable: drives both response-payload schema capture and the
/// `Err` short-circuit in `apply_response_shapers`, and a non-`Result`
/// caller cannot accidentally match.
pub(crate) fn result_inner(ty: &Type) -> Option<&Type> {
    nth_generic_type(ty, "Result", 0)
}

/// The JSON payload type of a handler's return — strips one optional `Result`
/// then a `Json`. Non-JSON returns yield `None`.
fn response_payload(output: &ReturnType) -> Option<Type> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    let inner = result_inner(ty).unwrap_or(ty);
    nth_generic_type(inner, "Json", 0).cloned()
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    // The `429` OAPI-O4 signal is detected on the guard path's last segment, so
    // it survives a fully-qualified path and a `use`-imported name alike, and a
    // guard that merely *contains* the substring does not false-positive.
    #[test]
    fn guard_path_is_throttler_matches_the_last_segment_only() {
        let plain: Path = parse_quote!(ThrottlerGuard);
        let qualified: Path = parse_quote!(nest_rs_throttler::ThrottlerGuard);
        let absolute: Path = parse_quote!(::nest_rs_throttler::ThrottlerGuard);
        assert!(guard_path_is_throttler(&plain));
        assert!(guard_path_is_throttler(&qualified));
        assert!(guard_path_is_throttler(&absolute));

        let other: Path = parse_quote!(AuthnGuard);
        let lookalike: Path = parse_quote!(MyThrottlerGuardWrapper);
        let module_named: Path = parse_quote!(ThrottlerGuard::helper);
        assert!(!guard_path_is_throttler(&other));
        assert!(!guard_path_is_throttler(&lookalike));
        // The *last* segment is `helper`, not the guard type — no match.
        assert!(!guard_path_is_throttler(&module_named));
    }
}
