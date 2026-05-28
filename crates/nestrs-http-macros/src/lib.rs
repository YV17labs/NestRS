//! HTTP decorator macros, re-exported by `nestrs-http` so apps write
//! `nestrs_http::controller` etc. The generated code uses absolute paths
//! (`::nestrs_http::*`, `::poem::*`, `::nestrs_core::*`), so this crate does
//! not depend on those crates — they resolve at the call site.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, Attribute, Expr, FnArg, ImplItem, ItemImpl, ItemStruct, Lit, LitStr, Meta,
    Path, ReturnType, Token, Type,
};

use nestrs_codegen::{
    build_injectable_body, dependencies_method, dependency_names_method, forwarded_arg_idents,
    from_container_method, impl_self_ident, injected_keys_expr, injected_method, nth_generic_type,
    optional_dependencies_method, InjectableBody,
};

/// One route handler in a controller: its HTTP verb ident, the generated
/// wrapper-fn ident, the guard paths declared with `#[use_guards]`, the filter
/// paths declared with `#[use_filters]`, the `Authorize<_, _>` parameter type
/// (if any) that drives response shaping, and the `#[meta(...)]` value
/// expressions attached to the route.
type RouteHandler = (
    syn::Ident,
    syn::Ident,
    Vec<Path>,
    Vec<Path>,
    Option<Type>,
    Vec<Expr>,
);

/// Handlers grouped by path in first-seen order — several verbs may share a path
/// (`GET` + `POST /users`), which `#[routes]` collapses into one `RouteMethod`.
type RoutesByPath = Vec<(LitStr, Vec<RouteHandler>)>;

/// `#[controller(path = "/health")]` — paired with `#[routes]` on the impl block.
///
/// Generates a `from_container(&Container) -> Self` constructor and a
/// `pub const PATH: &'static str` used by `#[routes]` as the route prefix.
///
/// An optional `version = "1"` enables **URI API versioning** for the whole
/// controller: every route mounts under `/v1` (the NestJS `@Version` analog with
/// `VersioningType.URI`). The version segment flows into the route log and the
/// OpenAPI document too — see [`version_path`](::nestrs_http::version_path).
///
/// The `Discoverable` impl is emitted by `#[routes]` rather than here — it
/// needs the route table that `#[routes]` collects, and emitting it in two
/// places would conflict.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let (path_lit, version) = match parse_controller_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let version_opt = match &version {
        Some(v) => quote! { ::core::option::Option::Some(#v) },
        None => quote! { ::core::option::Option::None },
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The controller's `#[inject]` keys for the access-graph check.
    // `#[controller]` sees the fields but `#[routes]` emits the
    // `Discoverable`, so expose them as an inherent fn `#[routes]` reads back
    // into `Discoverable::injected`.
    let injected_keys = injected_keys_expr(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            pub const PATH: &'static str = #path_lit;
            pub const VERSION: ::core::option::Option<&'static str> = #version_opt;

            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
            }
        }
    }
    .into()
}

/// Mark a struct as an HTTP interceptor that the framework will discover
/// and wrap around the route tree.
///
/// Behaves like `#[injectable]` for construction (fields with `#[inject]`
/// pulled from the container, others default), and additionally emits an
/// `impl Discoverable` that attaches an `HttpInterceptorMeta` describing
/// this type. The HTTP transport reads those metas via
/// `DiscoveryService::meta::<HttpInterceptorMeta>()` at boot.
///
/// The struct must implement `nestrs_middleware::Interceptor` — the macro
/// emits an `Arc<dyn Interceptor>` cast that fails at compile time if it
/// does not.
#[proc_macro_attribute]
pub fn interceptor(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let dependencies = dependencies_method(&dep_keys);
    let dependency_names = dependency_names_method(&dep_names);
    let optional_dependencies = optional_dependencies_method(&opt_keys);
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                let __snapshot = builder.snapshot();
                let __value = Self::from_container(&__snapshot);
                let __arc: ::std::sync::Arc<dyn ::nestrs_middleware::Interceptor> =
                    ::std::sync::Arc::new(__value);
                builder.attach_meta::<Self, ::nestrs_http::HttpInterceptorMeta>(
                    ::nestrs_http::HttpInterceptorMeta::new(__arc),
                )
            }
        }
    }
    .into()
}

/// Bind controller methods to HTTP routes.
///
/// Applied to an `impl` block belonging to a `#[controller]`-marked struct.
/// Each method tagged with `#[get("/path")]`, `#[post("/path")]`, `#[put]`,
/// `#[delete]` or `#[patch]` is wired as a poem handler. Method signatures
/// keep `&self` plus any poem extractors (`Path<T>`, `Json<T>`, `Query<T>`...).
///
/// Tag a method with `#[use_guards(GuardA, GuardB)]` to run those guards before
/// it — each is resolved from the container (so a guard is an `#[injectable]`
/// provider that can inject its own dependencies) and the first listed runs
/// outermost. A guard may attach request-scoped context the handler reads back
/// via `nestrs_http::Ctx<T>`. Like the verb attributes, `#[use_guards]` is
/// consumed here and needs no import.
///
/// Tag a method with `#[use_filters(FilterA, FilterB)]` to bind exception filters
/// to just that route (the `@UseFilters` analog; `HttpTransport::filter` is the
/// global form). Each is resolved from the container and wraps the handler
/// *outside* its guards, so it maps an error from the handler or a guard into a
/// response. Consumed here like `#[use_guards]`, no import needed.
///
/// Tag a method with `#[meta(EXPR)]` (repeatable) to attach a typed metadata
/// value to the route — the `@SetMetadata` / `@Roles` analog. `EXPR` is
/// evaluated once at mount and inserted into the request just outside the
/// route's guards, so a `#[use_guards]` guard reads it back with
/// `nestrs_http::Reflector::new(req).get::<T>()` to vary its decision. The value
/// type must be `Clone + Send + Sync + 'static`. Like `#[use_guards]`, the
/// attribute is consumed here and needs no import.
///
/// Tag a method with `#[api(summary = "...", description = "...", tags("a",
/// "b"))]` to enrich its OpenAPI operation (the analog of NestJS's
/// `@ApiOperation` / `@ApiTags`); every field is optional and, like
/// `#[use_guards]`, the attribute is consumed here. Independently, the macro
/// reads each handler's signature and records the schema of any `Json<T>`
/// request body or response into the route's [`HttpRouteMeta`], so an OpenAPI
/// generator can describe the payloads with no extra annotation. `T` must
/// implement `nestrs_http::schemars::JsonSchema` (handlers returning a raw
/// `Response`/`String` carry no schema and need no such bound).
///
/// Emits two impls on the controller:
/// - `nestrs_http::Controller` — the mount entry point used by the HTTP
///   transport.
/// - `nestrs_core::Discoverable` — attaches an `HttpControllerMeta` that
///   carries the declarative route table (verb + path + handler name) plus
///   a closure capturing the typed mount logic. The transport iterates
///   these metas at boot.
#[proc_macro_attribute]
pub fn routes(_args: TokenStream, input: TokenStream) -> TokenStream {
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

        // `#[use_guards(GuardA, GuardB)]` next to the verb attribute, consumed
        // here like the verbs are. The guards are resolved from the container at
        // mount time and wrapped around this handler's endpoint.
        let guards: Vec<Path> = match method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("use_guards"))
        {
            Some(g_idx) => {
                let g_attr = method.attrs.remove(g_idx);
                match g_attr.parse_args_with(
                    syn::punctuated::Punctuated::<Path, syn::Token![,]>::parse_terminated,
                ) {
                    Ok(paths) => paths.into_iter().collect(),
                    Err(err) => return err.to_compile_error().into(),
                }
            }
            None => Vec::new(),
        };

        // `#[use_filters(FilterA, FilterB)]` next to the verb attribute, consumed
        // here like `#[use_guards]`. Each filter is resolved from the container at
        // mount time and wraps this handler *outside* its guards, so it maps an
        // error from the handler (or a guard) to a response — the per-route
        // `@UseFilters` analog (`HttpTransport::filter` stays the global form).
        let filters: Vec<Path> = match method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("use_filters"))
        {
            Some(f_idx) => {
                let f_attr = method.attrs.remove(f_idx);
                match f_attr.parse_args_with(
                    syn::punctuated::Punctuated::<Path, syn::Token![,]>::parse_terminated,
                ) {
                    Ok(paths) => paths.into_iter().collect(),
                    Err(err) => return err.to_compile_error().into(),
                }
            }
            None => Vec::new(),
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

    // One `.at(path, RouteMethod)` per path: the first verb opens the
    // `RouteMethod`, the rest chain onto it (`get(h1).post(h2)`).
    let route_entries: Vec<TokenStream2> =
        routes_by_path
            .iter()
            .map(|(path, handlers)| {
                let mut handlers = handlers.iter();
                let (
                    first_verb,
                    first_wrapper,
                    first_guards,
                    first_filters,
                    first_shaper,
                    first_metas,
                ) = handlers.next().expect("each path has at least one verb");
                let first = guarded_handler(
                    first_wrapper,
                    first_guards,
                    first_filters,
                    first_shaper,
                    first_metas,
                );
                let mut method = quote! { ::poem::#first_verb(#first) };
                for (verb, wrapper, guards, filters, shaper, metas) in handlers {
                    let ep = guarded_handler(wrapper, guards, filters, shaper, metas);
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
                let __prefix = ::nestrs_http::version_path(<#self_ty>::VERSION, <#self_ty>::PATH);
                route.nest(__prefix.as_str(), __sub)
            }
        }

        impl ::nestrs_core::Discoverable for #self_ty {
            // The controller is built at mount time, so `dependencies` (register
            // ordering) stays empty; `injected` reports its `#[inject]` keys for
            // the access-graph check, read from the inherent
            // fn `#[controller]` emits.
            fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
                <#self_ty>::__nestrs_injected()
            }

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

/// Wrap a route handler in its response shaper (if any), its `#[use_guards]`
/// guards, its `#[use_filters]` exception filters, and its `#[meta(...)]` route
/// metadata. From inner to outer: shaper → guards → filters → metadata. The
/// shaper sits innermost — *inside* the guards — so a guard that attached request
/// context (the authorization ability) has run before the shaper's `capture`.
/// Filters wrap *outside* the guards so a filter maps an error from the handler
/// or a guard to a response; each guard/filter is resolved from the container at
/// mount time, first listed ending up outermost within its layer. The metadata
/// values wrap outermost, so each is inserted into the request before any guard's
/// `check` and a guard reads it back with `nestrs_http::Reflector`. Generated
/// inside `Controller::mount`, where `container: &Container` is in scope.
fn guarded_handler(
    wrapper: &syn::Ident,
    guards: &[Path],
    filters: &[Path],
    shaper: &Option<Type>,
    metas: &[Expr],
) -> TokenStream2 {
    let mut expr = match shaper {
        Some(ty) => quote! {
            ::nestrs_http::shaped(#wrapper, ::core::marker::PhantomData::<#ty>)
        },
        None => quote! { #wrapper },
    };
    for g in guards.iter().rev() {
        expr = quote! {
            ::nestrs_http::EndpointExt::guard(
                #expr,
                ::nestrs_core::Container::get::<#g>(container).expect(concat!(
                    "#[use_guards] guard `",
                    stringify!(#g),
                    "` is not registered — add it to a module's providers"
                )),
            )
        };
    }
    for f in filters.iter().rev() {
        expr = quote! {
            ::nestrs_http::EndpointExt::filter(
                #expr,
                ::nestrs_core::Container::get::<#f>(container).expect(concat!(
                    "#[use_filters] filter `",
                    stringify!(#f),
                    "` is not registered — add it to a module's providers"
                )),
            )
        };
    }
    // Outermost: the value is evaluated once here at mount and inserted into the
    // request (its extensions) before the guards run, where `Reflector` reads it.
    for m in metas {
        expr = quote! { ::poem::EndpointExt::data(#expr, #m) };
    }
    expr
}

// OpenAPI capture: `#[api(...)]` parsing and `Json<T>` payload-type extraction.

/// Parse `#[controller(path = "...", version = "1")]` — `path` required,
/// `version` optional (URI API versioning, the `@Controller({ version })`
/// analog). Order-independent; an unknown key is rejected with a clear message.
fn parse_controller_args(args: TokenStream2) -> syn::Result<(LitStr, Option<LitStr>)> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut path = None;
    let mut version = None;
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => path = Some(expr_str(&nv.value)?),
            Meta::NameValue(nv) if nv.path.is_ident("version") => {
                version = Some(expr_str(&nv.value)?)
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[controller] accepts `path = \"...\"` and an optional `version = \"...\"`",
                ))
            }
        }
    }
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[controller] requires `path = \"...\"`",
        )
    })?;
    Ok((path, version))
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

/// A `key = "..."` value must be a string literal.
fn expr_str(expr: &Expr) -> syn::Result<LitStr> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}

/// `Some(lit)` → `Some("lit")` tokens, `None` → `None` tokens.
fn opt_str(value: &Option<LitStr>) -> TokenStream2 {
    match value {
        Some(lit) => quote! { ::core::option::Option::Some(#lit) },
        None => quote! { ::core::option::Option::None },
    }
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
