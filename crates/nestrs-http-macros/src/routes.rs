//! `#[routes]` — bind a `#[controller]` impl block's verb-tagged methods to HTTP
//! routes, emitting the `Controller` mount + `Discoverable` impls, and capturing
//! per-route OpenAPI metadata.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, Attribute, Expr, FnArg, ImplItem, ItemImpl, LitStr, Meta, Path, ReturnType,
    Token, Type,
};

use nestrs_codegen::{
    forwarded_arg_idents, impl_self_ident, injected_method_with_layers, layer_inject_keys,
    nth_generic_type,
};

use crate::attr::{expr_str, opt_str, take_use_attr};

/// One route handler in a controller: its HTTP verb ident, the generated
/// wrapper-fn ident, the guard paths declared with `#[use_guards]`, the filter
/// paths declared with `#[use_filters]`, the interceptor paths declared with
/// `#[use_interceptors]`, the `Authorize<_, _>` parameter type (if any) that
/// drives response shaping, and the `#[meta(...)]` value expressions attached to
/// the route.
type RouteHandler = (
    syn::Ident,
    syn::Ident,
    Vec<Path>,
    Vec<Path>,
    Vec<Path>,
    Option<Type>,
    Vec<Expr>,
);

/// Handlers grouped by path in first-seen order — several verbs may share a path
/// (`GET` + `POST /users`), which `#[routes]` collapses into one `RouteMethod`.
type RoutesByPath = Vec<(LitStr, Vec<RouteHandler>)>;

pub(crate) fn routes(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    // The controller struct name doubles as the default OpenAPI tag, so routes
    // group by controller in the docs unless `#[api(tags(...))]` overrides it.
    let ctrl_name = match impl_self_ident(&self_ty, "routes") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let ctrl_tag = LitStr::new(&ctrl_name.to_string(), ctrl_name.span());

    let mut wrappers: Vec<TokenStream2> = Vec::new();
    // Verbs grouped by path, in first-seen order. poem rejects two `.at(path,..)`
    // for the same path, so several verbs on one path (REST `GET`+`POST /users`)
    // must collapse into a single `RouteMethod` (`get(h1).post(h2)`).
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

        wrappers.push(quote! {
            #[::poem::handler]
            async fn #wrapper_name(
                ::poem::web::Data(__ctrl): ::poem::web::Data<&::std::sync::Arc<#self_ty>>
                #extra_inputs
            ) -> #return_type {
                __ctrl.#method_name(#(#arg_idents),*).await
            }
        });

        // `#[use_guards]` / `#[use_filters]` / `#[use_interceptors]` beside the verb
        // attribute, each consumed here like the verb is and resolved from the
        // container at mount time. They nest around the handler per
        // `guarded_handler`: a guard gates access, a filter wraps *outside* the
        // guards (mapping a handler/guard error to a response), an interceptor
        // *inside* them (so a guard may short-circuit first). A bindable
        // interceptor is a plain `#[injectable] + impl Interceptor`; `#[interceptor]`
        // stays the global auto-discovered form.
        let guards = match take_use_attr(&mut method.attrs, "use_guards") {
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

        // `#[meta(EXPR)]` next to the verb attribute (repeatable) — a typed value
        // inserted into the request just outside this route's guards, so a
        // `#[use_guards]` guard reads it back with `nestrs_http::Reflector` to
        // vary its decision (the `@Roles` / `@SetMetadata` analog).
        let mut metas: Vec<Expr> = Vec::new();
        while let Some(m_idx) = method.attrs.iter().position(|a| a.path().is_ident("meta")) {
            let m_attr = method.attrs.remove(m_idx);
            match m_attr.parse_args::<Expr>() {
                Ok(expr) => metas.push(expr),
                Err(err) => return err.to_compile_error().into(),
            }
        }

        // A handler that declares an `Authorize<A, S>` parameter has its
        // response shaped (field-masked) by that gate type — detected by name so
        // this crate emits only `::nestrs_http::shaped` plus the app's own type,
        // never a path into the authz crate.
        let shaper = shaper_type(&inputs);

        let handler = (
            verb_ident.clone(),
            wrapper_name.clone(),
            guards,
            filters,
            interceptors,
            shaper.clone(),
            metas,
        );
        match routes_by_path
            .iter_mut()
            .find(|(path, _)| path.value() == route_path.value())
        {
            Some((_, handlers)) => handlers.push(handler),
            None => routes_by_path.push((route_path.clone(), vec![handler])),
        }

        let verb_variant = match verb_ident.to_string().as_str() {
            "get" => quote!(::nestrs_http::HttpVerb::Get),
            "post" => quote!(::nestrs_http::HttpVerb::Post),
            "put" => quote!(::nestrs_http::HttpVerb::Put),
            "delete" => quote!(::nestrs_http::HttpVerb::Delete),
            "patch" => quote!(::nestrs_http::HttpVerb::Patch),
            _ => unreachable!("verb_ident filtered above"),
        };

        // `#[api(summary = "...", tags(...))]` — optional OpenAPI metadata,
        // consumed here like `#[use_guards]`. Absent fields fall back below.
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

        // Capture the JSON request body / response payload schemas. Each emits
        // `schema_of::<T>` (a `JsonSchema` bound on `T`); a non-JSON payload
        // emits `None` and imposes no bound.
        let request_body = match request_payload(&inputs) {
            Some(ty) => quote! {
                ::core::option::Option::Some(::nestrs_http::schema_of::<#ty> as ::nestrs_http::SchemaFn)
            },
            None => quote! { ::core::option::Option::None },
        };
        // A shaped (masked) response has no static schema — the fields it
        // carries depend on the caller's ability — so skip schema capture there.
        let response = match (shaper.is_some(), response_payload(&method.sig.output)) {
            (false, Some(ty)) => quote! {
                ::core::option::Option::Some(::nestrs_http::schema_of::<#ty> as ::nestrs_http::SchemaFn)
            },
            _ => quote! { ::core::option::Option::None },
        };

        route_metas.push(quote! {
            ::nestrs_http::HttpRouteMeta {
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

    // Per-route guards / filters / interceptors fold into the controller's
    // access-graph dependencies: each is container-resolved at mount, so a layer
    // registered in a non-imported module must fail the boot with the same
    // `AccessGraphError` an out-of-reach `#[inject]` gets — not be resolved
    // silently through the flat container. Combined here (across every handler)
    // with the controller-level layers `#[controller]` already folded into
    // `__nestrs_injected`.
    let route_layer_keys = layer_inject_keys(
        routes_by_path
            .iter()
            .flat_map(|(_, handlers)| handlers.iter())
            .flat_map(|(_, _, guards, filters, interceptors, _, _)| {
                guards.iter().chain(filters).chain(interceptors)
            }),
    );
    let injected_method = injected_method_with_layers(&self_ty, &route_layer_keys);

    // One `.at(path, RouteMethod)` per path: the first verb opens the
    // `RouteMethod`, the rest chain onto it (`get(h1).post(h2)`).
    let route_entries: Vec<TokenStream2> = routes_by_path
        .iter()
        .map(|(path, handlers)| {
            let mut handlers = handlers.iter();
            let (
                first_verb,
                first_wrapper,
                first_guards,
                first_filters,
                first_interceptors,
                first_shaper,
                first_metas,
            ) = handlers.next().expect("each path has at least one verb");
            let first = guarded_handler(
                first_wrapper,
                first_guards,
                first_filters,
                first_interceptors,
                first_shaper,
                first_metas,
            );
            let mut method = quote! { ::poem::#first_verb(#first) };
            for (verb, wrapper, guards, filters, interceptors, shaper, metas) in handlers {
                let ep = guarded_handler(wrapper, guards, filters, interceptors, shaper, metas);
                method = quote! { #method.#verb(#ep) };
            }
            quote! { .at(#path, #method) }
        })
        .collect();

    quote! {
        #item

        #(#wrappers)*

        impl ::nestrs_http::Controller for #self_ty {
            fn mount(
                container: &::nestrs_core::Container,
                route: ::poem::Route,
            ) -> ::poem::Route {
                use ::poem::EndpointExt;
                let __ctrl = ::std::sync::Arc::new(<#self_ty>::from_container(container));
                let __sub = ::poem::Route::new()
                    #(#route_entries)*
                    .data(__ctrl);
                // Wrap the whole subtree in the controller-level layers
                // (interceptors → guards → filters; a no-op when none are
                // declared); they sit outside every per-route layer.
                let __sub = <#self_ty>::__nestrs_controller_layers(container, __sub);
                let __prefix = ::nestrs_http::version_path(<#self_ty>::VERSION, <#self_ty>::PATH);
                route.nest(__prefix.as_str(), __sub)
            }
        }

        impl ::nestrs_core::Discoverable for #self_ty {
            // The controller is built at mount time, so `dependencies` (register
            // ordering) stays empty; `injected` reports its `#[inject]` keys *and*
            // every container-resolved layer (controller-level, from the inherent
            // fn `#[controller]` emits, plus the per-route guards/filters/
            // interceptors gathered here) for the access-graph check.
            #injected_method

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                let __meta = ::nestrs_http::HttpControllerMeta::new(
                    <#self_ty>::PATH,
                    <#self_ty>::VERSION,
                    ::std::vec![#(#route_metas),*],
                    |__c, __r| <#self_ty as ::nestrs_http::Controller>::mount(__c, __r),
                );
                builder.attach_meta::<#self_ty, ::nestrs_http::HttpControllerMeta>(__meta)
            }
        }
    }
    .into()
}

/// The `Authorize<A, S>` parameter type a handler declares, if any. Found by the
/// last path segment being `Authorize` with angle-bracketed arguments, so the
/// macro stays free of any compile dependency on the authz crate. The first such
/// parameter wins; importing `Authorize` under an alias is not detected, so
/// response shaping is silently skipped for it.
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

/// Wrap a route handler in its response shaper (if any), its `#[use_interceptors]`
/// interceptors, its `#[use_guards]` guards, its `#[use_filters]` exception
/// filters, and its `#[meta(...)]` route metadata. From inner to outer:
/// shaper → interceptors → guards → filters → metadata. The shaper sits
/// innermost — *inside* the guards — so a guard that attached request context
/// (the authorization ability) has run before the shaper's `capture`.
/// Interceptors sit just outside the shaper but *inside* the guards, so a guard
/// runs (and may short-circuit) before an interceptor's pre-handler work — the
/// NestJS lifecycle order (guards before interceptors), the per-route mirror of
/// it. Filters wrap *outside* the guards so a filter maps an error from the
/// handler or a guard to a response; each interceptor/guard/filter is resolved
/// from the container at mount time, first listed ending up outermost within its
/// layer. The metadata values wrap outermost, so each is inserted into the
/// request before any guard's `check` and a guard reads it back with
/// `nestrs_http::Reflector`. Generated inside `Controller::mount`, where
/// `container: &Container` is in scope.
fn guarded_handler(
    wrapper: &syn::Ident,
    guards: &[Path],
    filters: &[Path],
    interceptors: &[Path],
    shaper: &Option<Type>,
    metas: &[Expr],
) -> TokenStream2 {
    let mut expr = match shaper {
        Some(ty) => quote! {
            ::nestrs_http::shaped(#wrapper, ::core::marker::PhantomData::<#ty>)
        },
        None => quote! { #wrapper },
    };
    // Inner → outer, the call order *is* the nesting order.
    expr = wrap_layer(expr, interceptors, "interceptor", "use_interceptors");
    expr = wrap_layer(expr, guards, "guard", "use_guards");
    expr = wrap_layer(expr, filters, "filter", "use_filters");
    // Outermost: the value is evaluated once here at mount and inserted into the
    // request (its extensions) before the guards run, where `Reflector` reads it.
    for m in metas {
        expr = quote! { ::poem::EndpointExt::data(#expr, #m) };
    }
    expr
}

/// Wrap a handler endpoint expression in a list of container-resolved layers via
/// the `EndpointExt::<kind>` extension (`interceptor` / `guard` / `filter`, the
/// method name equal to `kind`). Reversed so the first-listed entry ends up
/// outermost within its layer; `attr` names the source attribute for the
/// not-registered diagnostic. Composes inline (no boxing) — the per-controller
/// counterpart that boxes to a stable type is `controller_layers` in `controller`.
fn wrap_layer(mut expr: TokenStream2, paths: &[Path], kind: &str, attr: &str) -> TokenStream2 {
    let method = format_ident!("{kind}");
    let prefix = format!("#[{attr}] {kind} `");
    for p in paths.iter().rev() {
        expr = quote! {
            ::nestrs_http::EndpointExt::#method(
                #expr,
                ::nestrs_core::Container::get::<#p>(container).expect(concat!(
                    #prefix,
                    stringify!(#p),
                    "` is not registered — add it to a module's providers"
                )),
            )
        };
    }
    expr
}

/// Parsed `#[api(...)]` facets. Everything is optional; an empty attribute (or
/// no attribute at all) leaves the route's OpenAPI defaults untouched.
#[derive(Default)]
struct ApiMeta {
    summary: Option<LitStr>,
    description: Option<LitStr>,
    tags: Vec<LitStr>,
}

/// Parse `#[api(summary = "...", description = "...", tags("a", "b"))]`.
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
                ))
            }
        }
    }
    Ok(out)
}

/// The JSON payload type behind an extractor parameter: `Json<T>`,
/// `Valid<Json<T>>`, and `Piped<_, Json<T>>` all yield `T`. Anything that does
/// not bottom out in `Json<…>` (a `Path<…>`, a `Ctx<…>`) yields `None`.
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

/// The first request-body payload among a handler's value parameters (the
/// receiver is already stripped before this is called).
fn request_payload(inputs: &[FnArg]) -> Option<Type> {
    inputs.iter().find_map(|arg| match arg {
        FnArg::Typed(pt) => json_payload(&pt.ty),
        _ => None,
    })
}

/// The JSON payload type of a handler's return, if any: strips one optional
/// `Result<…>` then a `Json<…>`. `Json<Vec<UserDto>>` and `Result<Json<UserDto>>`
/// both resolve to the type inside `Json<…>` (`Vec<UserDto>` / `UserDto`); a
/// non-JSON return (`Response`, `String`) yields `None`.
fn response_payload(output: &ReturnType) -> Option<Type> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    let inner = nth_generic_type(ty, "Result", 0).unwrap_or(ty);
    nth_generic_type(inner, "Json", 0).cloned()
}
