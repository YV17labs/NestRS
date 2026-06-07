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
    forwarded_arg_idents, impl_self_ident, injected_method_with_layers, layer_inject_keys,
    nth_generic_type,
};

use crate::attr::{expr_str, opt_str, take_flag_attr, take_use_attr};

/// One route handler: verb ident, wrapper-fn ident, `#[use_guards]` paths,
/// `#[use_filters]` paths, `#[use_interceptors]` paths, the `Authorize<_, _>`
/// shaper type (if any), `#[meta(...)]` value expressions, the `#[public]`
/// opt-out flag, the `#[no_pipes]` opt-out flag, and `#[force_guards]` paths.
type RouteHandler = (
    syn::Ident,
    syn::Ident,
    Vec<Path>,
    Vec<Path>,
    Vec<Path>,
    Option<Type>,
    Vec<Expr>,
    bool,
    bool,
    Vec<Path>,
);

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

        let inputs: Vec<FnArg> = method.sig.inputs.iter().skip(1).cloned().collect();
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

        let guards = match take_use_attr(&mut method.attrs, "use_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let force_guards = match take_use_attr(&mut method.attrs, "force_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let filters = match take_use_attr(&mut method.attrs, "use_filters") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let interceptors = match take_use_attr(&mut method.attrs, "use_interceptors") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[public]` opts out of `Auth` + `Authorization` global layers on
        // this route; `Validation`, `Observability`, `RateLimit` keep running.
        let is_public = take_flag_attr(&mut method.attrs, "public");
        // `#[no_pipes]` opts out of every global pipe for this route.
        let no_pipes = take_flag_attr(&mut method.attrs, "no_pipes");

        // Drained after the `use_*` attributes so error spans for a misuse of
        // a response decorator point past the layers — and *before* emitting
        // the wrapper fn so the wrapper's return type and body reflect any
        // status / header / redirect override. The method's block is forwarded
        // so `#[redirect]` can reject a non-empty body (which the macro would
        // silently drop).
        let response_decorators = match crate::response_decorators::take_response_decorators(
            &mut method.attrs,
            &method.block,
        ) {
            Ok(d) => d,
            Err(err) => return err.to_compile_error().into(),
        };

        let call_expr = quote! { __ctrl.#method_name(#(#arg_idents),*).await };
        let returns_result = match &method.sig.output {
            ReturnType::Type(_, ty) => result_inner(ty).is_some(),
            ReturnType::Default => false,
        };
        let (wrapper_return_type, wrapper_body) = if response_decorators.is_empty() {
            (return_type.clone(), call_expr)
        } else {
            let mut wrapper_args: Vec<syn::Ident> = Vec::with_capacity(arg_idents.len() + 1);
            wrapper_args.push(syn::Ident::new("__ctrl", proc_macro2::Span::call_site()));
            wrapper_args.extend(arg_idents.iter().cloned());
            let body = crate::response_decorators::apply_response_decorators(
                &response_decorators,
                call_expr,
                &wrapper_args,
                returns_result,
            );
            (
                quote! { ::poem::Result<::poem::Response> },
                body,
            )
        };

        wrappers.push(quote! {
            #[::poem::handler]
            async fn #wrapper_name(
                ::poem::web::Data(__ctrl): ::poem::web::Data<&::std::sync::Arc<#self_ty>>
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

        let handler = (
            verb_ident.clone(),
            wrapper_name.clone(),
            guards,
            filters,
            interceptors,
            shaper.clone(),
            metas,
            is_public,
            no_pipes,
            force_guards,
        );
        match routes_by_path
            .iter_mut()
            .find(|(path, _)| path.value() == route_path.value())
        {
            Some((_, handlers)) => handlers.push(handler),
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
            }
        });
    }

    // Per-route layers fold into the access-graph dependencies so an unimported
    // module fails boot with an `AccessGraphError`, not a silent resolution.
    let route_layer_keys = layer_inject_keys(
        routes_by_path
            .iter()
            .flat_map(|(_, handlers)| handlers.iter())
            .flat_map(|(_, _, guards, filters, interceptors, _, _, _, _, force_guards)| {
                guards
                    .iter()
                    .chain(filters)
                    .chain(interceptors)
                    .chain(force_guards)
            }),
    );
    let injected_method = injected_method_with_layers(&self_ty, &route_layer_keys);

    let route_entries: Vec<TokenStream2> = routes_by_path
        .iter()
        .map(|(path, handlers)| {
            let mut iter = handlers.iter();
            let first = iter.next().expect("each path has at least one verb");
            let first_label = format!("{} {}", first.0, path.value());
            let first_ep = guarded_handler(first, &first_label);
            let first_verb = &first.0;
            let mut method = quote! { ::poem::#first_verb(#first_ep) };
            for handler in iter {
                let label = format!("{} {}", handler.0, path.value());
                let ep = guarded_handler(handler, &label);
                let verb = &handler.0;
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
                route: ::poem::Route,
            ) -> ::poem::Route {
                use ::poem::EndpointExt;
                let __ctrl = ::std::sync::Arc::new(<#self_ty>::from_container(container));
                let __sub = ::poem::Route::new()
                    #(#route_entries)*
                    .data(__ctrl);
                let __sub = <#self_ty>::__nestrs_controller_layers(container, __sub);
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
                    <#self_ty>::PATH,
                    <#self_ty>::VERSION,
                    ::std::vec![#(#route_metas),*],
                    |__c, __r| <#self_ty as ::nest_rs_http::Controller>::mount(__c, __r),
                );
                builder.attach_meta::<#self_ty, ::nest_rs_http::HttpControllerMeta>(__meta)
            }
        }
    }
    .into()
}

/// The `Authorize<A, S>` parameter type, found by the last path segment being
/// `Authorize` with angle-bracketed arguments — no compile dep on the authz
/// crate. Aliased imports are not detected and shaping is silently skipped.
fn shaper_type(inputs: &[FnArg]) -> Option<Type> {
    inputs.iter().find_map(|arg| {
        let FnArg::Typed(pt) = arg else { return None };
        let Type::Path(tp) = pt.ty.as_ref() else {
            return None;
        };
        let last = tp.path.segments.last()?;
        match last.ident == "Authorize"
            && matches!(last.arguments, syn::PathArguments::AngleBracketed(_))
        {
            true => Some((*pt.ty).clone()),
            false => None,
        }
    })
}

/// Build one routed handler. Layout, inner → outer:
///
/// shaper → per-route interceptors → per-route filters → metadata data →
/// `LayersRouteInterceptor` (Layer System dedup of global + per-route guards
/// and pipes).
///
/// Per-route guards are NOT inline-wrapped any more — they're handed to the
/// `LayersRouteInterceptor` as `RouteGuardSpec` entries so the interceptor
/// can dedup them against the global chain by `TypeId` and run the result in
/// canonical category order.
fn guarded_handler(handler: &RouteHandler, route_label: &str) -> TokenStream2 {
    let (
        _verb,
        wrapper,
        guards,
        filters,
        interceptors,
        shaper,
        metas,
        is_public,
        no_pipes,
        force_guards,
    ) = handler;
    let mut expr = match shaper {
        Some(ty) => quote! {
            ::nest_rs_http::shaped(#wrapper, ::core::marker::PhantomData::<#ty>)
        },
        None => quote! { #wrapper },
    };
    // Inner → outer: call order is the nesting order.
    expr = wrap_layer(expr, interceptors, "interceptor", "use_interceptors");
    expr = wrap_layer(expr, filters, "filter", "use_filters");

    // LayersRouteInterceptor sits *inside* the metadata wrap so per-route
    // guards reading `#[meta(...)]` via `Reflector` see it; outside the
    // per-route `use_interceptors`/`use_filters` wraps so a denial
    // short-circuits before any handler-side work.
    let route_label_lit = LitStr::new(route_label, proc_macro2::Span::call_site());
    let method_guard_specs = guard_specs(guards);
    let force_guard_typeids = force_guard_typeids(force_guards);
    let _ = no_pipes; // wire-through reserved for the future no-pipes spec
    expr = quote! {
        ::nest_rs_http::EndpointExt::interceptor(
            #expr,
            ::std::sync::Arc::new(
                ::nest_rs_guards::LayersRouteInterceptor::new(
                    #route_label_lit,
                    ::std::vec![],
                    #method_guard_specs,
                    #force_guard_typeids,
                )
            ),
        )
    };

    // Metadata is attached *after* the LayersRouteInterceptor so per-route
    // guards see the route's `#[meta]` value when the chain runs.
    for m in metas {
        expr = quote! { ::poem::EndpointExt::data(#expr, #m) };
    }

    // `#[public]` attaches a `Public` marker as route data. The framework
    // does not act on it; guards read it via `Reflector::is_public()` and
    // adjust their own policy.
    if *is_public {
        expr = quote! {
            ::poem::EndpointExt::data(#expr, ::nest_rs_core::Public)
        };
    }

    expr
}

/// Build the `Vec<RouteGuardSpec>` for the method-level guards. Each entry
/// captures the type id + resolver fn — the interceptor calls them at first
/// request.
fn guard_specs(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec![] };
    }
    let entries = paths.iter().map(|p| {
        quote! {
            ::nest_rs_guards::integration::RouteLayerSpec {
                type_id: ::core::any::TypeId::of::<#p>(),
                name: ::core::any::type_name::<#p>(),
                resolve: |__c| ::nest_rs_core::Container::get::<#p>(__c)
                    .map(|__arc| __arc as ::std::sync::Arc<dyn ::nest_rs_guards::Guard>),
            }
        }
    });
    quote! { ::std::vec![#(#entries),*] }
}

fn force_guard_typeids(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec![] };
    }
    let entries = paths.iter().map(|p| {
        quote! { ::core::any::TypeId::of::<#p>() }
    });
    quote! { ::std::vec![#(#entries),*] }
}

/// Wrap a handler in container-resolved layers via `EndpointExt::<kind>`.
/// Composes inline (no boxing); the controller-level counterpart that boxes to
/// a stable type is `controller_layers` in `controller`.
fn wrap_layer(mut expr: TokenStream2, paths: &[Path], kind: &str, attr: &str) -> TokenStream2 {
    let method = format_ident!("{kind}");
    let prefix = format!("#[{attr}] {kind} `");
    for p in paths.iter().rev() {
        expr = quote! {
            ::nest_rs_http::EndpointExt::#method(
                #expr,
                ::nest_rs_core::Container::get::<#p>(container).expect(concat!(
                    #prefix,
                    stringify!(#p),
                    "` is not registered — add it to a module's providers"
                )),
            )
        };
    }
    expr
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

/// `Some(T)` when `ty` is `Result<T, _>`, `None` otherwise. Detects the
/// unqualified last-segment ident `Result` — it does not resolve type
/// aliases (proc-macros have no name resolution), so a feature-local
/// alias whose last segment is `Result` is matched while a renamed
/// `type Outcome<T, E> = Result<T, E>;` is not. That limitation is
/// acceptable: drives both response-payload schema capture and the
/// `Err` short-circuit in `apply_response_decorators`, and a non-`Result`
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
