//! `#[routes]` — bind a `#[controller]` impl block's verb-tagged methods to
//! HTTP routes, with their mount, discovery and OpenAPI metadata.

use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, ImplItem, LitStr, Meta, Path, ReturnType, Token, Type, parse_quote,
};

use nest_rs_codegen::{
    Collision, Conditional, DispatchKeys, HostBorrow, RoutePath, await_if_async, cfg_attrs,
    force_guard_typeids, guard_capability_bounds, impl_self_ident, injected_methods_with_layers,
    layer_deps, mixed_site_ident, normalize_forwarded_args, nth_generic_type, scoped_specs,
    shared_receiver, take_flag_attr, take_path_list,
};

use crate::attr::opt_str;

/// One route handler.
struct RouteHandler {
    verb: syn::Ident,
    wrapper: syn::Ident,
    /// Whether the verb was `#[sse]`.
    is_sse: bool,
    /// `#[use_guards]` paths on the method.
    guards: Vec<Path>,
    /// `#[use_filters]` paths on the method.
    filters: Vec<Path>,
    /// `#[use_interceptors]` paths on the method.
    interceptors: Vec<Path>,
    /// Every declared parameter type, in order: arming a shaper is type-directed.
    param_types: Vec<Type>,
    /// A parameter *spelled* `Authorize<..>` / `Bind<..>`, kept only so one not
    /// implementing the shaper trait is a spanned compile error.
    named_shaper: Option<Type>,
    /// Whether the handler declares any extractor; without one, no mask probe is emitted.
    has_extractors: bool,
    metas: Vec<Expr>,
    is_public: bool,
    no_pipes: bool,
    /// `#[force_guards]` paths on the method.
    force_guards: Vec<Path>,
    /// `#[use_pipes]` paths on the method.
    pipes: Vec<Path>,
    /// `#[use_exception_filters]` paths on the method.
    exception_filters: Vec<Path>,
    /// The subset of the controller's versions this route serves; empty means all.
    versions: Vec<LitStr>,
    /// The method's `#[cfg]` conditions, carried onto every item emitted for the
    /// route outside the method.
    cfgs: Vec<TokenStream2>,
}

/// Handlers grouped by address in first-seen order: poem rejects two
/// `.at(path, ..)` for one path, so the verbs of an address share one method table.
///
/// Grouped by [`RoutePath::identity`], never by the path as written: `/p/:id`
/// and `/p/:other/` are one address.
type RoutesByPath = Vec<RouteGroup>;

/// One address and every handler serving it.
struct RouteGroup {
    /// [`RoutePath::identity`] — what makes two paths one address.
    identity: String,
    /// The path poem mounts.
    mount: LitStr,
    /// The method that first declared the address, for the refusal that names it.
    first: syn::Ident,
    handlers: Vec<RouteHandler>,
}

pub(crate) fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::HTTP
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

/// What `#[routes]` consumes off a method beside the layers and the posture.
const HELPERS: [&str; 12] = [
    "get",
    "post",
    "put",
    "delete",
    "patch",
    "sse",
    "no_pipes",
    "http_code",
    "response_header",
    "redirect",
    "crud_write",
    "crud_location",
];

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = pair::HTTP.reject_args(
        &TokenStream2::from(args),
        "a controller's `path` and `version` are declared by",
    ) {
        return err.to_compile_error().into();
    }
    let mut item = match pair::HTTP.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    if let Err(err) = pair::HTTP.reject_host_layers(&item.attrs) {
        return err.to_compile_error().into();
    }
    let self_ty = item.self_ty.clone();

    // Default OpenAPI tag, unless `#[api(tags(...))]` overrides.
    let ctrl_name = match impl_self_ident(&self_ty, "#[routes]") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let ctrl_tag = LitStr::new(&ctrl_name.to_string(), ctrl_name.span());
    // The controller half of an OpenAPI `operationId`, computed at compile time:
    // the runtime cannot reach `nest_rs_codegen` without pulling `syn` into every app.
    let ctrl_token = LitStr::new(&controller_token(&ctrl_name.to_string()), ctrl_name.span());

    let mut wrappers: Vec<TokenStream2> = Vec::new();
    let mut routes_by_path: RoutesByPath = Vec::new();
    let mut routes_declared = DispatchKeys::new(
        "#[routes]",
        "a controller serves each verb and path with one handler in each version, so the second \
         would never run — give one a distinct path, verb or `#[version]`",
    );
    let mut route_metas: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index = match nest_rs_codegen::one_role_per_method(
            "verb",
            &method.attrs,
            &["get", "post", "put", "delete", "patch", "sse"],
            "",
        ) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };
        let attr = method.attrs.remove(index);
        let Some(declared_verb) = attr.path().get_ident().cloned() else {
            continue;
        };
        // Before the arguments are read: without `&self`, the first argument is
        // not a receiver.
        if let Err(err) = shared_receiver(method, "#[routes]", &ctrl_name, HostBorrow::Host)
            .and_then(|()| nest_rs_codegen::concrete_signature(method, "#[routes]"))
        {
            return err.to_compile_error().into();
        }
        // The route's wrapper, mount and document entry are compiled out with the method.
        let cfgs = cfg_attrs(&method.attrs);
        // `#[sse]` is a `GET`, so `#[sse("/x")]` beside `#[get("/x")]` is a duplicate route.
        let is_sse = declared_verb == "sse";
        let verb_ident = if is_sse {
            format_ident!("get", span = declared_verb.span())
        } else {
            declared_verb.clone()
        };

        let written_path = match route_path(&attr, &declared_verb) {
            Ok(path) => path,
            Err(err) => return err.to_compile_error().into(),
        };
        let parsed_path = match RoutePath::parse(&written_path.value()) {
            Ok(parsed) => parsed,
            Err(why) => {
                return syn::Error::new_spanned(
                    &written_path,
                    format!(
                        "{}: {:?} is not a path poem can mount: {why}",
                        nest_rs_codegen::site(&declared_verb.to_string(), None),
                        written_path.value()
                    ),
                )
                .to_compile_error()
                .into();
            }
        };
        let route_path = LitStr::new(parsed_path.mount(), written_path.span());

        let method_name = method.sig.ident.clone();
        let method_name_lit = method_name.to_string();
        // Qualified by the controller: each wrapper is a module-level type, and two
        // controllers in one file (`V1Controller::list`, `V2Controller::list`) share it.
        let wrapper_name = format_ident!("__nestrs_route_{}_{}", ctrl_name, method_name);

        let mut inputs: Vec<FnArg> = method.sig.inputs.iter().skip(1).cloned().collect();
        // Before the `#[authorize]` insert below, whose wrapper-only parameter is
        // never forwarded.
        let arg_idents = match normalize_forwarded_args(&mut inputs) {
            Ok(idents) => idents,
            Err(err) => return err.to_compile_error().into(),
        };
        let authorize = match take_authorize(&mut method.attrs) {
            Ok(spec) => spec,
            Err(err) => return err.to_compile_error().into(),
        };
        if is_sse && let Some(spec) = &authorize {
            return syn::Error::new_spanned(
                &spec.action,
                "`#[authorize(...)]` cannot arm a `#[sse]` route: the posture masks the \
                 response against the entity model, and an event stream is no wire model to \
                 reconcile. Gate the stream with a capability-only guard instead — \
                 `#[use_guards(YourGuard)]` checking `ability.can_class(...)`",
            )
            .to_compile_error()
            .into();
        }
        // A hand-written `Authorize<A, E>` or `Bind<A, S>` arms the shaper too, and
        // the mask passes `text/event-stream` through untouched.
        if is_sse && let Some(ty) = named_shaper_type(&inputs) {
            return syn::Error::new_spanned(
                &ty,
                "a response shaper cannot arm a `#[sse]` route: masking reconciles the body \
                 against the entity model, and an event stream is no wire model to reconcile — \
                 it would be waved through unmasked while the document claims otherwise. Gate \
                 the stream with a capability-only guard instead, and load what it needs inside \
                 the handler",
            )
            .to_compile_error()
            .into();
        }
        if let Some(spec) = &authorize {
            match authorize_param(spec, &inputs) {
                Ok(param) => inputs.insert(0, param),
                Err(err) => return err.to_compile_error().into(),
            }
        }
        let return_type = match &method.sig.output {
            ReturnType::Default => quote! { () },
            ReturnType::Type(_, ty) => quote! { #ty },
        };

        let guards = match take_path_list(&mut method.attrs, "use_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let method_guarded = !guards.is_empty();
        let method_throttled = guards.iter().any(guard_path_is_throttler);
        let force_guards = match take_path_list(&mut method.attrs, "force_guards") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let filters = match take_path_list(&mut method.attrs, "use_filters") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let interceptors = match take_path_list(&mut method.attrs, "use_interceptors") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let method_pipes = match take_path_list(&mut method.attrs, "use_pipes") {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        let method_exception_filters =
            match take_path_list(&mut method.attrs, "use_exception_filters") {
                Ok(paths) => paths,
                Err(err) => return err.to_compile_error().into(),
            };
        // Global guards still run on a `#[public]` route.
        let is_public = match take_flag_attr(&mut method.attrs, "public") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };
        if is_public && let Some(spec) = &authorize {
            return syn::Error::new_spanned(
                &spec.action,
                "a route is `#[public]` or `#[authorize(...)]`, never both — the two \
                 declare opposite postures",
            )
            .to_compile_error()
            .into();
        }
        let method_versions = match take_version_attr(&mut method.attrs) {
            Ok(versions) => versions,
            Err(err) => return err.to_compile_error().into(),
        };
        let no_pipes = match take_flag_attr(&mut method.attrs, "no_pipes") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[crud]`'s write-op marker: the document advertises the `409` they can answer.
        let may_conflict = match take_flag_attr(&mut method.attrs, "crud_write") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[crud]`'s create marker: the document declares the `Location` it sends.
        let mut sets_location = match take_flag_attr(&mut method.attrs, "crud_location") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };

        let response_shapers =
            match crate::response::take_response_shapers(&mut method.attrs, &method.block) {
                Ok(d) => d,
                Err(err) => return err.to_compile_error().into(),
            };
        if is_sse && !response_shapers.is_empty() {
            return syn::Error::new_spanned(
                &declared_verb,
                "`#[sse]` takes no response decorator — `#[http_code]`, `#[redirect]` and \
                 `#[response_header]` all shape a response that completes, and an event \
                 stream does not. The route answers `200 text/event-stream` for as long as \
                 it streams",
            )
            .to_compile_error()
            .into();
        }
        let success_status = response_shapers.success_status();
        if response_shapers.redirect.is_some() {
            sets_location = true;
            method.attrs.push(parse_quote! {
                #[allow(dead_code, reason = "#[redirect] answers without calling the handler")]
            });
        }

        // Mixed-site, so a handler parameter spelled `req`, `body`, `__ctrl` or `res`
        // cannot shadow the wrapper's own (`Json(body): Json<T>` masked the request body).
        let req_var = mixed_site_ident("req");
        let body_var = mixed_site_ident("body");
        let ctrl_var = mixed_site_ident("__ctrl");
        let res_var = mixed_site_ident("res");

        let call_expr = await_if_async(
            &method.sig,
            // By path, never `__ctrl.method(..)`: method lookup tries the `Arc` before it
            // derefs, so a same-named trait method on `Arc<T>` would run instead.
            quote! { <#self_ty>::#method_name(&**#ctrl_var, #(#arg_idents),*) },
        );
        let (wrapper_return_type, wrapper_body) = if is_sse {
            // A fallible open is told by type through `nest_rs_core::Answer`. The return
            // type is inferred: no macro can name the handler's `impl Stream`.
            let wrapped = quote! {{
                use ::nest_rs_core::AnswerFallback as _;
                let __nestrs_answer = #call_expr;
                ::nest_rs_core::Answer(&__nestrs_answer)
                    .map::<::nest_rs_http::poem::Error, _, _>()(
                    __nestrs_answer,
                    |__nestrs_stream| {
                        ::nest_rs_http::SseSettings::respond(&__nestrs_sse, __nestrs_stream)
                    },
                )
            }};
            (quote! { _ }, wrapped)
        } else if response_shapers.is_empty() {
            (return_type.clone(), call_expr)
        } else {
            let mut wrapper_args: Vec<syn::Ident> = Vec::with_capacity(arg_idents.len() + 1);
            wrapper_args.push(ctrl_var.clone());
            wrapper_args.extend(arg_idents.iter().cloned());
            let body = crate::response::apply_response_shapers(
                &response_shapers,
                call_expr,
                &wrapper_args,
            );
            (
                quote! { ::nest_rs_http::poem::Result<::nest_rs_http::poem::Response> },
                body,
            )
        };

        // Mirrors poem's `#[handler]` expansion, except the controller `Arc` is a
        // captured field rather than a per-request `Data` extension.
        let extractor_stmts: Vec<TokenStream2> = inputs
            .iter()
            .filter_map(|arg| match arg {
                FnArg::Typed(pt) => {
                    let pat = &pt.pat;
                    let ty = &pt.ty;
                    Some(quote! {
                        let #pat = <#ty as ::nest_rs_http::poem::FromRequest>::from_request(
                            &#req_var, &mut #body_var,
                        )
                        .await?;
                    })
                }
                FnArg::Receiver(_) => None,
            })
            .collect();
        // Copied out of `self` before the async block so the future captures a
        // value rather than borrowing the endpoint.
        let (sse_field, sse_binding) = if is_sse {
            (
                quote! { __sse: ::nest_rs_http::SseSettings, },
                quote! { let __nestrs_sse = self.__sse; },
            )
        } else {
            (quote! {}, quote! {})
        };
        wrappers.push(quote! {
            #(#cfgs)*
            #[allow(non_camel_case_types)]
            struct #wrapper_name {
                __ctrl: ::std::sync::Arc<#self_ty>,
                #sse_field
            }

            #(#cfgs)*
            impl ::nest_rs_http::poem::Endpoint for #wrapper_name {
                type Output = ::nest_rs_http::poem::Response;

                #[allow(unused_mut)]
                async fn call(
                    &self,
                    mut #req_var: ::nest_rs_http::poem::Request,
                ) -> ::nest_rs_http::poem::Result<Self::Output> {
                    let (#req_var, mut #body_var) = #req_var.split();
                    #(#extractor_stmts)*
                    let #ctrl_var = &self.__ctrl;
                    #sse_binding
                    let #res_var: #wrapper_return_type = async move { #wrapper_body }.await;
                    let #res_var = ::nest_rs_http::poem::error::IntoResult::into_result(#res_var);
                    ::std::result::Result::map(
                        #res_var,
                        ::nest_rs_http::poem::IntoResponse::into_response,
                    )
                }
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

        // The compiler selects the arming parameter, so this crate needs no authz
        // dependency and a renamed import still arms.
        let param_types = param_types(&inputs);
        let shaper_selection = shaper_selection(&param_types);

        let handler = RouteHandler {
            verb: verb_ident.clone(),
            is_sse,
            wrapper: wrapper_name.clone(),
            guards,
            filters,
            interceptors,
            param_types: param_types.clone(),
            named_shaper: named_shaper_type(&inputs),
            has_extractors: !inputs.is_empty(),
            metas,
            is_public,
            no_pipes,
            force_guards,
            pipes: method_pipes,
            exception_filters: method_exception_filters,
            versions: method_versions.clone(),
            cfgs: cfgs.clone(),
        };
        // poem silently keeps the last of two handlers for one (verb, path); refused
        // here without `#[cfg]`, and by rustc through the marker when both compile.
        let served: Vec<String> = method_versions.iter().map(LitStr::value).collect();
        let route = match served.as_slice() {
            [] => format!(
                "{} {}",
                verb_ident.to_string().to_uppercase(),
                route_path.value()
            ),
            versions => format!(
                "{} {} #[version({})]",
                verb_ident.to_string().to_uppercase(),
                route_path.value(),
                versions
                    .iter()
                    .map(|version| format!("{version:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        };
        if let Err(err) = routes_declared.declare_in(
            Collision::Marker,
            "route",
            // The verb folds, the path does not: the marker's hash keeps `/Users` and
            // `/users` apart.
            &format!("{} {}", verb_ident, parsed_path.identity()),
            &served,
            &route,
            &method_name,
            &cfgs,
            &attr,
        ) {
            return err.to_compile_error().into();
        }
        match routes_by_path
            .iter_mut()
            .find(|group| group.identity == parsed_path.identity())
        {
            // poem binds each parameter by the name the address was first mounted under.
            Some(group) if group.mount.value() != route_path.value() => {
                return syn::Error::new_spanned(
                    &written_path,
                    format!(
                        "`#[routes]` mounts one address as `{}` on `{}` and as `{}` on `{}`: poem \
                         mounts an address once and binds each parameter by the name it was \
                         mounted under, so one handler would read a parameter that is not there \
                         — name the parameters alike on both",
                        group.mount.value(),
                        group.first,
                        route_path.value(),
                        method_name,
                    ),
                )
                .to_compile_error()
                .into();
            }
            Some(group) => group.handlers.push(handler),
            None => routes_by_path.push(RouteGroup {
                identity: parsed_path.identity().to_owned(),
                mount: route_path.clone(),
                first: method_name.clone(),
                handlers: vec![handler],
            }),
        }

        let verb_variant = match verb_ident.to_string().as_str() {
            "get" => quote!(::nest_rs_http::HttpVerb::Get),
            "post" => quote!(::nest_rs_http::HttpVerb::Post),
            "put" => quote!(::nest_rs_http::HttpVerb::Put),
            "delete" => quote!(::nest_rs_http::HttpVerb::Delete),
            "patch" => quote!(::nest_rs_http::HttpVerb::Patch),
            _ => unreachable!("verb_ident filtered above"),
        };

        let api = match nest_rs_codegen::take_single_attr(&mut method.attrs, "api") {
            Ok(Some(a_attr)) => match parse_api_attr(&a_attr) {
                Ok(api) => api,
                Err(err) => return err.to_compile_error().into(),
            },
            Ok(None) => ApiMeta::default(),
            Err(err) => return err.to_compile_error().into(),
        };
        let summary = opt_str(&api.summary);
        let description = opt_str(&api.description);
        let tags = if api.tags.is_empty() {
            quote! { &[#ctrl_tag] }
        } else {
            let tags = &api.tags;
            quote! { &[#(#tags),*] }
        };

        let json_body = first_extractor_payload(&inputs, "Json");
        let form_body = first_extractor_payload(&inputs, "Form");
        // A route has one request body, so the ways to declare one are mutually exclusive.
        let declared: Vec<(&str, &dyn ToTokens)> = [
            api.multipart
                .as_ref()
                .map(|ty| ("`#[api(multipart = …)]`", ty as &dyn ToTokens)),
            json_body
                .as_ref()
                .map(|ty| ("a `Json<…>` extractor", ty as &dyn ToTokens)),
            form_body
                .as_ref()
                .map(|ty| ("a `Form<…>` extractor", ty as &dyn ToTokens)),
        ]
        .into_iter()
        .flatten()
        .collect();
        if let [(first, _), (second, tokens)] = declared[..] {
            return syn::Error::new_spanned(
                tokens,
                format!(
                    "a route has one request body, and this handler declares two: {first} and \
                     {second} — keep one",
                ),
            )
            .to_compile_error()
            .into();
        }
        let request_body = match (&api.multipart, &json_body) {
            (Some(ty), _) => quote! {
                ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Multipart(
                    ::core::option::Option::Some(
                        ::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
                    ),
                ))
            },
            (None, Some(ty)) => quote! {
                ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Json(
                    ::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
                ))
            },
            (None, None) if takes_multipart(&inputs) => quote! {
                ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Multipart(
                    ::core::option::Option::None,
                ))
            },
            (None, None) => match &form_body {
                Some(ty) => quote! {
                    ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Form(
                        ::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
                    ))
                },
                None => quote! { ::core::option::Option::None },
            },
        };
        // Kept under a shaper: masking returns a subset of this shape, recorded as `masked`.
        let response = match api
            .response
            .clone()
            .or_else(|| response_payload(&method.sig.output))
        {
            Some(ty) => quote! {
                ::core::option::Option::Some(::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn)
            },
            None => quote! { ::core::option::Option::None },
        };
        if is_sse && let Some(lit) = &api.response_content_type {
            return syn::Error::new_spanned(
                lit,
                "`#[sse]` already answers `text/event-stream`; a `response_content_type` here \
                 can only make the document describe something the route does not send",
            )
            .to_compile_error()
            .into();
        }
        let response_content_type = match &api.response_content_type {
            Some(lit) => quote! { ::core::option::Option::Some(#lit) },
            None if is_sse || returns_sse(&method.sig.output) => {
                quote! { ::core::option::Option::Some("text/event-stream") }
            }
            None => quote! { ::core::option::Option::None },
        };
        let masked = quote! { #shaper_selection.is_some() };

        let path_param_tys = path_param_types(&inputs);
        let path_params = if path_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#path_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };
        let query_param_tys = extractor_payloads(&inputs, "Query");
        let query_params = if query_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#query_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };
        let header_param_tys = extractor_payloads(&inputs, "Header");
        let header_params = if header_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#header_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };

        let route_versions = quote! { &[#(#method_versions),*] };
        // A version the controller never declared would mount nowhere: the
        // transport loops over the controller's versions.
        if let Some(first) = method_versions.first() {
            let span = first.span();
            let message = LitStr::new(
                &format!(
                    "`#[version]` on `{}` names a version its `#[controller]` does not declare \
                     — add it to `#[controller(version = [..])]` or fix the spelling",
                    method_name_lit,
                ),
                span,
            );
            wrappers.push(quote_spanned! {span=>
                #(#cfgs)*
                const _: () = ::core::assert!(
                    ::nest_rs_http::versions_declare(<#self_ty>::VERSIONS, #route_versions),
                    #message,
                );
            });
        }

        route_metas.push(quote! {
            #(#cfgs)*
            ::nest_rs_http::HttpRouteMeta {
                verb: #verb_variant,
                path: #route_path,
                handler: #method_name_lit,
                summary: #summary,
                description: #description,
                tags: #tags,
                request_body: #request_body,
                response: #response,
                response_content_type: #response_content_type,
                masked: #masked,
                path_params: #path_params,
                query_params: #query_params,
                header_params: #header_params,
                may_conflict: #may_conflict,
                throttled: #method_throttled
                    || <#self_ty>::__nestrs_controller_has_throttler(),
                sets_location: #sets_location,
                success_status: #success_status,
                scoped_guarded: #method_guarded
                    || !<#self_ty>::__nestrs_controller_guard_specs().is_empty(),
                public: #is_public,
                versions: #route_versions,
            }
        });
    }

    // A layer from an unimported module fails boot with an `AccessGraphError`
    // rather than resolving through the flat container.
    let route_layers = layer_deps(
        routes_by_path
            .iter()
            .flat_map(|group| group.handlers.iter())
            .flat_map(|handler| {
                handler
                    .guards
                    .iter()
                    .chain(&handler.filters)
                    .chain(&handler.interceptors)
                    .chain(&handler.force_guards)
                    .chain(&handler.pipes)
                    .chain(&handler.exception_filters)
                    .map(|item| Conditional {
                        cfgs: &handler.cfgs,
                        item,
                    })
            }),
    );
    let injected_methods = injected_methods_with_layers(&self_ty, &route_layers);

    // `Guard::check_http` defaults to `Ok(())`, so a guard without the HTTP
    // capability would pass every request.
    let capability_bounds = guard_capability_bounds(
        routes_by_path
            .iter()
            .flat_map(|group| group.handlers.iter())
            .flat_map(|handler| {
                handler
                    .guards
                    .iter()
                    .chain(&handler.force_guards)
                    .map(|item| Conditional {
                        cfgs: &handler.cfgs,
                        item,
                    })
            }),
        quote!(::nest_rs_guards::HttpGuard),
    );

    // Only when the controller streams: an unused binding warns in the developer's build.
    let has_sse = routes_by_path
        .iter()
        .flat_map(|group| group.handlers.iter())
        .any(|handler| handler.is_sse);
    let sse_resolve = if has_sse {
        quote! { let __sse = ::nest_rs_http::SseSettings::resolve(container); }
    } else {
        quote! {}
    };

    // Built inside the per-version loop; a path is claimed only if a verb survived,
    // since an empty method table answers `405`. A `MethodTable` records the verb
    // set a `405`'s `Allow` owes (RFC 9110 §15.5.6).
    let route_entries: Vec<TokenStream2> = routes_by_path
        .iter()
        .map(|group| {
            let path = &group.mount;
            let arms: Vec<TokenStream2> = group
                .handlers
                .iter()
                .map(|handler| {
                    let label = format!("{} {}", handler.verb, path.value());
                    let ep = guarded_handler(handler, &label, &self_ty);
                    let verb = &handler.verb;
                    let versions = &handler.versions;
                    let cfgs = &handler.cfgs;
                    let serves = if versions.is_empty() {
                        quote! { true }
                    } else {
                        quote! {
                            match __version {
                                ::core::option::Option::Some(__v) => {
                                    [#(#versions),*].contains(&__v)
                                }
                                ::core::option::Option::None => true,
                            }
                        }
                    };
                    // A `let`, because a `#[cfg]` may sit on a statement and not
                    // on an `if` expression.
                    quote! {
                        #(#cfgs)*
                        let () = if #serves {
                            __method = __method.#verb(#ep);
                        };
                    }
                })
                .collect();
            quote! {
                {
                    #[allow(unused_mut)]
                    let mut __method = ::nest_rs_http::MethodTable::new();
                    #(#arms)*
                    if !__method.is_empty() {
                        __route = __route.at(
                            ::nest_rs_http::join_path(&__prefix, #path),
                            __method.into_endpoint(),
                        );
                    }
                }
            }
        })
        .collect();

    let markers = routes_declared.markers(&self_ty, &item.generics);

    quote! {
        #item

        #markers

        #capability_bounds

        #(#wrappers)*

        impl ::nest_rs_http::Controller for #self_ty {
            // Mounted flat: poem's `nest` re-slices and re-routes on every request.
            fn mount(
                container: &::nest_rs_core::Container,
                route: ::nest_rs_http::poem::Route,
            ) -> ::nest_rs_http::poem::Route {
                let __ctrl = ::std::sync::Arc::new(<#self_ty>::from_container(container));
                #sse_resolve
                let mut __route = route;
                // `[None]` for an unversioned controller: an empty list would unmount it.
                let __versions: ::std::vec::Vec<::core::option::Option<&'static str>> =
                    if <#self_ty>::VERSIONS.is_empty() {
                        ::std::vec![::core::option::Option::None]
                    } else {
                        <#self_ty>::VERSIONS
                            .iter()
                            .map(|__v| ::core::option::Option::Some(*__v))
                            .collect()
                    };
                for __version in __versions {
                    let __prefix =
                        ::nest_rs_http::version_path(__version, <#self_ty>::PATH);
                    #(#route_entries)*
                }
                __route
            }
        }

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_methods

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                let __meta = ::nest_rs_http::HttpControllerMeta::new(
                    #ctrl_tag,
                    #ctrl_token,
                    <#self_ty>::PATH,
                    <#self_ty>::VERSIONS,
                    ::std::vec![#(#route_metas),*],
                    |__c, __r| <#self_ty as ::nest_rs_http::Controller>::mount(__c, __r),
                );
                builder
                    .attach_meta::<#self_ty, ::nest_rs_http::HttpControllerMeta>(__meta)
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

/// Take `#[version("2")]` / `#[version("1", "2")]` off a route method.
fn take_version_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<LitStr>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("version")) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if let Some(second) = attrs.iter().find(|a| a.path().is_ident("version")) {
        return Err(syn::Error::new_spanned(
            second,
            "a route declares its versions in one `#[version(...)]`, listing them together",
        ));
    }
    nest_rs_codegen::versioning::parse_version_args(&attr)
}

/// Take `#[authorize(Action, Entity)]` off a route method.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn take_authorize(attrs: &mut Vec<Attribute>) -> syn::Result<Option<AuthorizeSpec>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("authorize")) else {
        return Ok(None);
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident("authorize")) {
        return Err(syn::Error::new_spanned(
            &attr,
            nest_rs_codegen::at_most_one_authorize("route"),
        ));
    }
    // Parsed as `Meta`, not `Path`, so `bind = Service` is refused by name rather
    // than by syn's `expected \`,\``.
    let metas: Vec<Meta> = attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|_| malformed_authorize(&attr))?
        .into_iter()
        .collect();
    for meta in &metas {
        let Meta::NameValue(nv) = meta else {
            continue;
        };
        if nv.path.is_ident("bind") {
            return Err(syn::Error::new_spanned(
                meta,
                nest_rs_codegen::posture_key_unsupported(
                    "bind = Service",
                    "HTTP",
                    "a route's subject is loaded by a `Bind<Service, Action>` parameter, which \
                     the handler declares and the compiler arms — so the binding is a *type* \
                     here, not an argument to this attribute",
                ),
            ));
        }
        if nv.path.is_ident("id_arg") {
            return Err(syn::Error::new_spanned(
                meta,
                nest_rs_codegen::posture_key_unsupported(
                    "id_arg = argument",
                    "HTTP",
                    nest_rs_codegen::ID_ARG_UNSUPPORTED_BECAUSE,
                ),
            ));
        }
    }
    let paths: Vec<Path> = metas
        .into_iter()
        .map(|meta| match meta {
            Meta::Path(path) => Ok(path),
            other => Err(syn::Error::new_spanned(other, malformed_authorize(&attr))),
        })
        .collect::<syn::Result<_>>()?;
    if let Some(unmasked) = paths.iter().find(|p| p.is_ident("unmasked")) {
        return Err(syn::Error::new_spanned(
            unmasked,
            nest_rs_codegen::posture_key_unsupported(
                "unmasked",
                "HTTP",
                "there is no value-level mask here to switch off. A route's response is \
                 shaped by a `RouteResponseShaper` the compiler arms from the *type* of \
                 the posture parameter `#[authorize]` emits, so what a body carries is \
                 decided by which extractor the handler declares",
            ),
        ));
    }
    let [action, entity] = <[Path; 2]>::try_from(paths).map_err(|_| malformed_authorize(&attr))?;
    Ok(Some(AuthorizeSpec { action, entity }))
}

/// The `#[authorize]` shape refusal.
fn malformed_authorize(attr: &Attribute) -> syn::Error {
    syn::Error::new_spanned(
        attr,
        "expected `#[authorize(Action, Entity)]` — e.g. \
         `#[authorize(Read, users::Entity)]`. Bind the subject with a \
         `Bind<Service, Action>` parameter when the route loads one",
    )
}

/// The extractor `#[authorize(Action, Entity)]` desugars to, the same one
/// `#[crud]` emits.
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

/// Whether a parameter is a hand-written `Authorize<..>`; a `Bind<..>` loads
/// the subject and stays.
fn is_authorize_param(arg: &FnArg) -> bool {
    let FnArg::Typed(pt) = arg else { return false };
    let Type::Path(tp) = pt.ty.as_ref() else {
        return false;
    };
    tp.path.segments.iter().any(|s| s.ident == "Authorize")
}

fn param_types(inputs: &[FnArg]) -> Vec<Type> {
    inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pt) => Some((*pt.ty).clone()),
            FnArg::Receiver(_) => None,
        })
        .collect()
}

/// The route's response shaper, selected by type after name resolution: the
/// first parameter that is a `RouteResponseShaper` arms the route, so an
/// aliased import arms too.
fn shaper_selection(param_types: &[Type]) -> TokenStream2 {
    if param_types.is_empty() {
        return quote! { ::core::option::Option::<::nest_rs_http::CaptureFn>::None };
    }
    quote! {{
        let mut __nestrs_shaper: ::core::option::Option<::nest_rs_http::CaptureFn> =
            ::core::option::Option::None;
        #(
            if __nestrs_shaper.is_none() {
                __nestrs_shaper = ::nest_rs_http::shaper_of!(#param_types);
            }
        )*
        __nestrs_shaper
    }}
}

/// A parameter *spelled* `Authorize<..>` / `Bind<..>`, for the diagnostic
/// only: arming is [`shaper_selection`]'s.
fn named_shaper_type(inputs: &[FnArg]) -> Option<Type> {
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

/// Build one routed handler. Layout, inner → outer: shaper (mask) →
/// exception filters → filters → interceptors → `RouteShaper` (guards + pipes)
/// → metadata. Global filters and interceptors run at the transport edge.
fn guarded_handler(handler: &RouteHandler, route_label: &str, self_ty: &Type) -> TokenStream2 {
    let RouteHandler {
        verb: _,
        versions: _,
        cfgs: _,
        is_sse,
        wrapper,
        guards,
        filters,
        interceptors,
        param_types,
        named_shaper,
        has_extractors,
        metas,
        is_public,
        no_pipes,
        force_guards,
        pipes: method_pipes,
        exception_filters: method_exception_filters,
    } = handler;
    let route_label_lit = LitStr::new(route_label, proc_macro2::Span::call_site());
    // Expanded inside `Controller::mount`, where `__ctrl` and `__sse` are in scope.
    let wrapper_expr = if *is_sse {
        quote! { #wrapper { __ctrl: ::std::sync::Arc::clone(&__ctrl), __sse: __sse } }
    } else {
        quote! { #wrapper { __ctrl: ::std::sync::Arc::clone(&__ctrl) } }
    };
    // The run-time probe catches a masking extractor reached indirectly (nested,
    // or a hand-rolled `FromRequest`), which no scan of the signature can see.
    let shaper_selection = shaper_selection(param_types);
    let named_shaper_assert = match named_shaper {
        Some(ty) => quote! {
            const _: fn() = || {
                fn __nestrs_assert_route_shaper<P: ::nest_rs_http::RouteResponseShaper>() {}
                __nestrs_assert_route_shaper::<#ty>();
            };
        },
        None => quote! {},
    };
    let probe = if *has_extractors {
        quote! { ::core::option::Option::Some(#route_label_lit) }
    } else {
        quote! { ::core::option::Option::None }
    };
    let mut expr = quote! {
        {
            #named_shaper_assert
            ::nest_rs_http::shaped(#wrapper_expr, #shaper_selection, #probe)
        }
    };
    let method_exception_filter_specs = scoped_specs(
        method_exception_filters,
        quote!(dyn ::nest_rs_exception_filters::ExceptionFilterErased),
    );
    let method_filter_specs = scoped_specs(filters, quote!(dyn ::nest_rs_filters::Filter));
    let method_interceptor_specs = scoped_specs(
        interceptors,
        quote!(dyn ::nest_rs_interceptors::Interceptor),
    );
    // One call for the three pools: each generic wrapper level adds a
    // `Request`-sized slot to the future poem boxes per request.
    expr = quote! {
        ::nest_rs_guards::dispatch::wrap_route_response_layers(
            container,
            #expr,
            &<#self_ty>::__nestrs_controller_exception_filter_specs(),
            &#method_exception_filter_specs,
            &<#self_ty>::__nestrs_controller_filter_specs(),
            &#method_filter_specs,
            &<#self_ty>::__nestrs_controller_interceptor_specs(),
            &#method_interceptor_specs,
            #route_label_lit,
        )
    };

    // Inside the metadata wrap, so guards reading `#[meta(...)]` see it.
    let method_guard_specs = scoped_specs(guards, quote!(dyn ::nest_rs_guards::Guard));
    let force_guard_typeids = force_guard_typeids(force_guards);
    let method_pipe_specs = scoped_specs(method_pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let no_pipes_flag = if *no_pipes {
        quote!(true)
    } else {
        quote!(false)
    };
    expr = quote! {
        ::nest_rs_guards::dispatch::wrap_route_shaper(
            container,
            #expr,
            #route_label_lit,
            <#self_ty>::__nestrs_controller_guard_specs(),
            #method_guard_specs,
            #force_guard_typeids,
            <#self_ty>::__nestrs_controller_pipe_specs(),
            #method_pipe_specs,
            #no_pipes_flag,
        )
    };

    for m in metas {
        expr = quote! { ::nest_rs_http::poem::EndpointExt::data(#expr, #m) };
    }

    // Guards read the marker via `Reflector::is_public()`.
    if *is_public {
        expr = quote! {
            ::nest_rs_http::poem::EndpointExt::data(#expr, ::nest_rs_http::Public)
        };
    }

    expr
}

/// A verb's one argument, the route's path, read as a string literal.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn route_path(attr: &Attribute, verb: &syn::Ident) -> syn::Result<LitStr> {
    let verb = verb.to_string();
    let refused = |at: &dyn ToTokens| {
        syn::Error::new_spanned(
            at,
            nest_rs_codegen::takes_value(
                &verb,
                None,
                &format!("the route's path as a string literal, e.g. `#[{verb}(\"/:id\")]`"),
            ),
        )
    };
    let written: Expr = attr.parse_args().map_err(|_| refused(attr))?;
    match nest_rs_codegen::ungrouped_expr(&written) {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(path),
            ..
        }) => Ok(path.clone()),
        other => Err(refused(other)),
    }
}

/// Whether a guard path's last segment names `ThrottlerGuard`, so the route
/// documents a `429`. By name: an alias is missed, a namesake counts.
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
    /// `#[api(response = T)]` — the payload the document advertises when the
    /// return type cannot state it.
    response: Option<Type>,
    /// `#[api(multipart = T)]` — the type describing the parts of a
    /// `multipart/form-data` body.
    multipart: Option<Type>,
    /// `#[api(response_content_type = "audio/mpeg")]` — the media type of a
    /// success body that is not JSON.
    response_content_type: Option<LitStr>,
}

/// Every key `#[api]` takes. A key the match in [`parse_api_attr`] does not
/// read is accepted and dropped: `every_api_key_is_read_as_the_table_writes_it`
/// holds the two together.
const API_KEYS: [&str; 6] = [
    "summary",
    "description",
    "tags",
    "response",
    "multipart",
    "response_content_type",
];

const API: nest_rs_codegen::Grammar = nest_rs_codegen::Grammar::new("api", &API_KEYS);

/// Parse `#[api(...)]` into [`ApiMeta`].
///
/// Read through [`Grammar`](nest_rs_codegen::Grammar), not `syn::Meta`, whose
/// `NameValue` reads `response = Vec<Post>` as chained comparisons.
fn parse_api_attr(attr: &Attribute) -> syn::Result<ApiMeta> {
    let mut out = ApiMeta::default();
    API.parse_attr(attr, |arg| {
        match arg.key() {
            "summary" => out.summary = Some(arg.str_lit("List users")?),
            "description" => out.description = Some(arg.str_lit("…")?),
            "response" => out.response = Some(api_type(arg.value()?, "response", "Vec<Post>")?),
            "multipart" => {
                out.multipart = Some(api_type(arg.value()?, "multipart", "UploadForm")?);
            }
            "response_content_type" => {
                let lit = arg.str_lit("text/csv")?;
                check_media_type(&lit)?;
                out.response_content_type = Some(lit);
            }
            "tags" => out.tags = api_tags(arg.input(), arg.ident())?,
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;
    Ok(out)
}

/// A type-valued `#[api]` key's value — `response = Vec<Post>`.
fn api_type(input: syn::parse::ParseStream<'_>, key: &str, example: &str) -> syn::Result<Type> {
    input.parse::<Type>().map_err(|stopped| {
        syn::Error::new(
            stopped.span(),
            nest_rs_codegen::takes_value(
                "api",
                Some(key),
                &format!("a type, e.g. `{key} = {example}`"),
            ),
        )
    })
}

/// `tags("a", "b")` — a list of string literals.
fn api_tags(input: syn::parse::ParseStream<'_>, key: &syn::Ident) -> syn::Result<Vec<LitStr>> {
    let refused = |at: &dyn ToTokens| {
        syn::Error::new_spanned(
            at,
            nest_rs_codegen::takes_value(
                "api",
                Some("tags"),
                "a list of string literals, e.g. `tags(\"users\", \"admin\")`",
            ),
        )
    };
    if !input.peek(syn::token::Paren) {
        return Err(refused(key));
    }
    let content;
    syn::parenthesized!(content in input);
    Punctuated::<Expr, Token![,]>::parse_terminated(&content)?
        .iter()
        .map(|tag| match nest_rs_codegen::ungrouped_expr(tag) {
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(tag),
                ..
            }) => Ok(tag.clone()),
            other => Err(refused(other)),
        })
        .collect()
}

/// The payload type behind an extractor named `name`: `Name<T>`,
/// `Valid<Name<T>>` and `Piped<_, Name<T>>` all yield `T`.
fn extractor_payload(ty: &Type, name: &str) -> Option<Type> {
    if let Some(payload) = nth_generic_type(ty, name, 0) {
        return Some(payload.clone());
    }
    if let Some(inner) = nth_generic_type(ty, "Valid", 0) {
        return extractor_payload(inner, name);
    }
    if let Some(inner) = nth_generic_type(ty, "Piped", 1) {
        return extractor_payload(inner, name);
    }
    None
}

/// Every `Name<T>` payload in the handler signature, in argument order.
fn extractor_payloads(inputs: &[FnArg], name: &str) -> Vec<Type> {
    inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pt) => extractor_payload(&pt.ty, name),
            _ => None,
        })
        .collect()
}

/// The first `Name<T>` payload in the handler signature.
fn first_extractor_payload(inputs: &[FnArg], name: &str) -> Option<Type> {
    extractor_payloads(inputs, name).into_iter().next()
}

/// The path-parameter types a handler binds, in path order: `Path<(A, B)>`
/// yields `[A, B]`, as poem binds tuple elements left to right.
fn path_param_types(inputs: &[FnArg]) -> Vec<Type> {
    match first_extractor_payload(inputs, "Path") {
        Some(Type::Tuple(tuple)) => tuple.elems.into_iter().collect(),
        Some(other) => vec![other],
        None => Vec::new(),
    }
}

/// Whether a type's last path segment is `name`, read off the spelling.
fn last_segment_is(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == name))
}

/// Whether the handler pulls the parts itself through poem's `Multipart`.
fn takes_multipart(inputs: &[FnArg]) -> bool {
    inputs.iter().any(|arg| match arg {
        FnArg::Typed(pt) => last_segment_is(&pt.ty, "Multipart"),
        FnArg::Receiver(_) => false,
    })
}

/// Whether the handler returns poem's `SSE`, possibly behind a `Result`.
fn returns_sse(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };
    last_segment_is(result_inner(ty).unwrap_or(ty), "SSE")
}

/// Refuse a malformed media type: it keys an OpenAPI `content` map no client
/// could match.
fn check_media_type(lit: &LitStr) -> syn::Result<()> {
    let value = lit.value();
    // Parameters (`; charset=utf-8`) aside.
    let essence = value.split(';').next().unwrap_or_default().trim();
    let mut halves = essence.split('/');
    let well_formed = match (halves.next(), halves.next(), halves.next()) {
        (Some(ty), Some(subtype), None) => {
            !ty.is_empty() && !subtype.is_empty() && !essence.chars().any(char::is_whitespace)
        }
        _ => false,
    };
    if !well_formed {
        return Err(syn::Error::new_spanned(
            lit,
            format!(
                "{}: {value:?} is not a media type — spell it `type/subtype`, e.g. \
                 \"application/octet-stream\", \"text/event-stream\" or \"audio/mpeg\"",
                nest_rs_codegen::site("api", Some("response_content_type")),
            ),
        ));
    }
    Ok(())
}

/// `Some(T)` when `ty` is spelled `Result<T, _>`. A reading of the spelling,
/// which an alias defeats: it decides only what the document infers, never what
/// the route does (that goes through `nest_rs_core::Answer`).
pub(crate) fn result_inner(ty: &Type) -> Option<&Type> {
    nth_generic_type(ty, "Result", 0)
}

/// The JSON payload type of a handler's return, under one optional `Result`.
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

    /// How each of [`API_KEYS`] is written, position for position.
    const API_KEYS_WRITTEN: [&str; 6] = [
        "summary = \"...\"",
        "description = \"...\"",
        "tags(\"a\", \"b\")",
        "response = Type",
        "multipart = Type",
        "response_content_type = \"type/subtype\"",
    ];

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
        assert!(!guard_path_is_throttler(&module_named));
    }

    fn api_attr(tokens: TokenStream2) -> syn::Result<ApiMeta> {
        let attr: Attribute = parse_quote!(#[api(#tokens)]);
        parse_api_attr(&attr)
    }

    #[test]
    fn api_response_accepts_a_generic_type_beside_the_string_arguments() {
        let meta = match api_attr(quote! {
            summary = "List Posts", tags("Post"), response = ::std::vec::Vec<Post>
        }) {
            Ok(meta) => meta,
            Err(err) => panic!("the argument list must parse: {err}"),
        };
        assert_eq!(
            meta.summary.map(|s| s.value()).as_deref(),
            Some("List Posts")
        );
        assert_eq!(meta.tags.len(), 1);
        let ty = meta.response.expect("a response type");
        assert_eq!(
            quote!(#ty).to_string(),
            quote!(::std::vec::Vec<Post>).to_string(),
        );
    }

    #[test]
    fn api_multipart_and_response_content_type_parse_beside_the_rest() {
        let meta = match api_attr(quote! {
            summary = "Upload",
            multipart = crate::dtos::UploadDto,
            response_content_type = "audio/mpeg"
        }) {
            Ok(meta) => meta,
            Err(err) => panic!("the argument list must parse: {err}"),
        };
        let ty = meta.multipart.expect("a multipart type");
        assert_eq!(
            quote!(#ty).to_string(),
            quote!(crate::dtos::UploadDto).to_string(),
        );
        assert_eq!(
            meta.response_content_type.map(|l| l.value()).as_deref(),
            Some("audio/mpeg"),
        );
    }

    #[test]
    fn a_response_content_type_that_is_not_a_media_type_is_rejected() {
        for bad in ["octet-stream", "audio/", "/mpeg", "audio / mpeg", "a/b/c"] {
            let msg = match api_attr(quote! { response_content_type = #bad }) {
                Ok(_) => panic!("`{bad}` must be rejected"),
                Err(err) => err.to_string(),
            };
            assert!(msg.contains("type/subtype"), "{msg}");
            assert!(
                msg.contains(bad),
                "the error quotes what was written: {msg}"
            );
        }
    }

    #[test]
    fn a_media_type_with_a_parameter_is_accepted() {
        api_attr(quote! { response_content_type = "text/event-stream; charset=utf-8" })
            .expect("a parameterized media type is still a media type");
    }

    #[test]
    fn sse_is_read_off_the_return_type_bare_or_behind_a_result() {
        let bare: ReturnType = parse_quote!(-> SSE);
        let qualified: ReturnType = parse_quote!(-> poem::web::sse::SSE);
        let behind_result: ReturnType = parse_quote!(-> Result<SSE>);
        assert!(returns_sse(&bare));
        assert!(returns_sse(&qualified));
        assert!(returns_sse(&behind_result));

        let json: ReturnType = parse_quote!(-> Json<Post>);
        let response: ReturnType = parse_quote!(-> Result<Response>);
        let nothing = ReturnType::Default;
        assert!(!returns_sse(&json));
        assert!(!returns_sse(&response));
        assert!(!returns_sse(&nothing));
    }

    #[test]
    fn a_payload_is_unwrapped_through_the_pipe_carriers() {
        for name in ["Header", "Json", "Form", "Query", "Path"] {
            let name_ident = format_ident!("{name}");
            let shapes: [Type; 3] = [
                parse_quote!(#name_ident<Tracing>),
                parse_quote!(Valid<#name_ident<Tracing>>),
                parse_quote!(Piped<Trim, #name_ident<Tracing>>),
            ];
            for ty in shapes {
                let inner = extractor_payload(&ty, name).expect("a payload");
                assert_eq!(quote!(#inner).to_string(), quote!(Tracing).to_string());
            }
            assert!(extractor_payload(&parse_quote!(Other<PageParams>), name).is_none());
        }
    }

    #[test]
    fn a_bare_multipart_parameter_is_detected_however_it_is_spelled() {
        let inputs: Vec<FnArg> = vec![
            parse_quote!(query: Query<TranscodeDto>),
            parse_quote!(form: poem::web::Multipart),
        ];
        assert!(takes_multipart(&inputs));
        assert!(!takes_multipart(&[parse_quote!(body: Json<Post>)]));
    }

    fn api_refusal(tokens: TokenStream2) -> String {
        match api_attr(tokens) {
            Ok(_) => panic!("the argument list must be refused"),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn an_unknown_api_argument_names_itself_and_the_accepted_set() {
        let expected = "unknown #[api] argument `returns`; expected `summary`, `description`, \
                        `tags`, `response`, `multipart` or `response_content_type`";
        assert_eq!(api_refusal(quote! { returns = Post }), expected);
        assert_eq!(api_refusal(quote! { returns }), expected);
    }

    #[test]
    fn a_bare_api_key_is_told_what_it_is_missing() {
        for (key, written) in API_KEYS.into_iter().zip(API_KEYS_WRITTEN) {
            let bare: TokenStream2 = key.parse().expect("a key is an identifier");
            let refusal = api_refusal(bare);
            if written.starts_with(&format!("{key} =")) {
                assert_eq!(
                    refusal,
                    format!("#[api] `{key}` needs a value — write `{key} = ...`"),
                );
            } else {
                assert!(
                    refusal.starts_with(&format!("#[api] `{key}` takes a list")),
                    "{refusal}"
                );
            }
        }
    }

    #[test]
    fn every_api_key_is_read_as_the_table_writes_it() {
        for (key, written) in API_KEYS.into_iter().zip(API_KEYS_WRITTEN) {
            assert!(
                written.starts_with(key),
                "`{written}` is the written form of `{key}`, position for position",
            );
            let tokens: TokenStream2 = written.parse().expect("the written form tokenizes");
            if let Err(err) = api_attr(tokens) {
                panic!("`{written}` must parse: {err}");
            }
        }
    }

    #[test]
    fn an_argument_that_is_not_a_key_is_named_as_written() {
        let refusal = api_refusal(quote! { "List users" });
        assert!(
            refusal.starts_with("unknown #[api] argument `\"List users\"`; expected `summary`"),
            "{refusal}"
        );
    }
}

const CONTROLLER_SUFFIX: &str = "Controller";

/// `PostsController` → `posts`. A name that is only the suffix keeps it.
fn controller_token(controller: &str) -> String {
    let stem = controller
        .strip_suffix(CONTROLLER_SUFFIX)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(controller);
    nest_rs_codegen::snake_case(stem)
}

#[cfg(test)]
mod token_tests {
    use super::controller_token;

    #[test]
    fn a_controller_is_named_by_what_it_serves() {
        assert_eq!(controller_token("PostsController"), "posts");
        assert_eq!(controller_token("HTTPProbeController"), "http_probe");
        assert_eq!(controller_token("Posts"), "posts");
        assert_eq!(controller_token("Controller"), "controller");
    }
}
