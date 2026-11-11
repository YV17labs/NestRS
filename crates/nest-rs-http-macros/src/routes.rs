//! `#[routes]` — bind a `#[controller]` impl block's verb-tagged methods to
//! HTTP routes, with their mount, discovery and OpenAPI metadata.

use nest_rs_codegen::__private::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned as _;
use syn::{
    Attribute, Expr, FnArg, ImplItem, LitStr, Meta, Path, ReturnType, Token, Type, parse_quote,
};

use nest_rs_codegen::__private::layers::{LayerScope, Layers, take_layers};
use nest_rs_codegen::__private::versioning;
use nest_rs_codegen::__private::{
    AUTHORIZED_NEEDS_BIND, BIND_NEEDS_AUTHORIZED, BIND_TAKES, Conditional, DispatchKeys,
    HandlerSite, HostBorrow, HostMeta, HostMount, ID_ARG_UNSUPPORTED_BECAUSE, RoutePath,
    SiteGuards, SiteItems, SitePosture, ambient_type_ids, authorized_entity, await_if_async,
    cfg_attrs, concrete_signature, deprecation_header, force_guard_typeids,
    guard_capability_bounds, handler_const, host_runtime_entry, impl_self_ident,
    injected_methods_with_layers, injected_per_request_method, layer_deps, mixed_site_ident,
    no_row_answered, normalize_forwarded_args, nth_generic_type, one_role_per_method,
    posture_key_unsupported, refuse_rust_deprecated, reject_meta_on_operations, scoped_specs,
    shared_receiver, site, site_items, site_ref, spelled, take_posture, takes_value,
    ungrouped_expr, unit_of_work_required, unknown_argument, validate_argument,
};
use nest_rs_codegen::take_flag_attr;

use crate::attr::opt_str;

/// One route handler.
struct RouteHandler {
    /// The `http::Method` constant the route answers (`GET`).
    method: syn::Ident,
    wrapper: syn::Ident,
    /// The `Deprecation` header value Rust's `#[deprecated]` on the method states.
    deprecation_header: Option<String>,
    /// Whether the verb was `#[sse]`.
    is_sse: bool,
    /// `#[use_guards]` paths on the method.
    guards: Vec<Path>,
    /// `#[use_interceptors]` paths on the method.
    interceptors: Vec<Path>,
    /// The type a posture naming a subject declares, then every parameter type,
    /// in order: arming a shaper and selecting a class gate are type-directed.
    probe_types: Vec<Type>,
    /// Every parameter type, in order, then the service a bound subject loads
    /// through: the providers resolved per request are read off them.
    param_types: Vec<Type>,
    /// The type a posture naming a subject declares, kept so a subject naming
    /// neither an entity nor a capability is a spanned compile error.
    posture_type: Option<Type>,
    /// On an `#[sse]` route, the assertion that its posture's subject shapes
    /// nothing, spanned on the subject the posture names.
    stream_posture: TokenStream2,
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
    /// Whether the route declares `#[authorize]`.
    authorized: bool,
    /// The handler method, which names the route's site.
    method_name: syn::Ident,
}

/// Handlers grouped by address in first-seen order: an address is mounted
/// once, so the verbs of an address share one method table.
///
/// Grouped by [`RoutePath::identity`], never by the path as written: `/p/{id}`
/// and `/p/{other}/` are one address.
type RoutesByPath = Vec<RouteGroup>;

/// One address and every handler serving it.
struct RouteGroup {
    /// [`RoutePath::identity`] — what makes two paths one address.
    identity: String,
    /// The template the address is mounted under.
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
const HELPERS: [&str; 11] = [
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
    if let Err(err) = pair::HTTP
        .reject_host_layers(&item.attrs)
        .and_then(|()| reject_meta_on_operations(&pair::HTTP, &item.attrs))
    {
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
    let mut handler_consts: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index = match one_role_per_method(
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
            .and_then(|()| concrete_signature(method, "#[routes]"))
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
        let method_const =
            syn::Ident::new(&verb_ident.to_string().to_uppercase(), verb_ident.span());

        let written_path = match route_path(&attr, &declared_verb) {
            Ok(path) => path,
            Err(err) => return err.to_compile_error().into(),
        };
        let parsed_path = match RoutePath::parse(&written_path.value()) {
            Ok(parsed) => parsed,
            Err(refusal) => {
                return syn::Error::new_spanned(
                    &written_path,
                    format!("{} {refusal}", site(&declared_verb.to_string(), None)),
                )
                .to_compile_error()
                .into();
            }
        };
        let route_path = LitStr::new(parsed_path.template(), written_path.span());

        let method_name = method.sig.ident.clone();
        let method_name_lit = method_name.to_string();
        // Qualified by the controller: each wrapper is a module-level type, and two
        // controllers in one file (`V1Controller::list`, `V2Controller::list`) share it.
        let wrapper_name = format_ident!("__nestrs_route_{}_{}", ctrl_name, method_name);
        let site_name = site_ref(&self_ty, &method_name);

        // `#[crud]`'s own parameters declare neither a gate nor a shaper, so
        // no probe asks their types again.
        let mut unprobed: Vec<usize> = Vec::new();
        for (index, arg) in method.sig.inputs.iter_mut().skip(1).enumerate() {
            if let FnArg::Typed(pt) = arg {
                match take_flag_attr(&mut pt.attrs, UNPROBED) {
                    Ok(true) => unprobed.push(index),
                    Ok(false) => {}
                    Err(err) => return err.to_compile_error().into(),
                }
            }
        }
        let mut inputs: Vec<FnArg> = method.sig.inputs.iter().skip(1).cloned().collect();
        let arg_idents = match normalize_forwarded_args(&mut inputs) {
            Ok(idents) => idents,
            Err(err) => return err.to_compile_error().into(),
        };
        let (authorize, is_public) = match take_posture(
            &mut method.attrs,
            &method.sig.ident,
            "route",
            |attr, word| gate_of(attr, word, &inputs),
        ) {
            Ok(posture) => posture,
            Err(err) => return err.to_compile_error().into(),
        };
        // Probed like a parameter and never extracted: the gate runs with the
        // guards, outside the route's own layers.
        let authorize_ty = authorize.as_ref().map(posture_type);
        // Decided by the posture's type, after name resolution: on a stream,
        // only a subject whose posture shapes nothing — one no table holds.
        let stream_posture = match (&authorize, &authorize_ty) {
            (Some(spec), Some(ty)) if is_sse => quote_spanned! {spec.entity.span()=>
                const _: () = ::core::assert!(
                    !<#ty as ::nest_rs_http::RouteResponseShaper>::SHAPES,
                    #SSE_NAMES_AN_ENTITY,
                );
            },
            _ => quote! {},
        };
        let return_type = match &method.sig.output {
            ReturnType::Default => quote! { () },
            ReturnType::Type(_, ty) => quote! { #ty },
        };

        let Layers {
            guards,
            force_guards,
            pipes: method_pipes,
            interceptors,
            exception_filters: method_exception_filters,
            meta: metas,
        } = match take_layers(&pair::HTTP, LayerScope::Method, &mut method.attrs) {
            Ok(layers) => layers,
            Err(err) => return err.to_compile_error().into(),
        };
        if let Err(err) = refuse_rust_deprecated(&method.attrs, "a `#[routes]` handler") {
            return err.to_compile_error().into();
        }
        let api = match nest_rs_codegen::take_single_attr(&mut method.attrs, "api") {
            Ok(Some(a_attr)) => match parse_api_attr(&a_attr) {
                Ok(api) => api,
                Err(err) => return err.to_compile_error().into(),
            },
            Ok(None) => ApiMeta::default(),
            Err(err) => return err.to_compile_error().into(),
        };
        let answered: Vec<&Type> = nest_rs_codegen::__private::return_type(&method.sig.output)
            .into_iter()
            .chain(api.response.as_ref())
            .collect();
        handler_consts.push(no_row_answered(&method_name, &cfgs, &answered));
        let deprecation = match api
            .deprecated
            .as_ref()
            .map(|since| deprecation_header(since).map(|header| (since.clone(), header)))
        {
            Some(Ok(read)) => Some(read),
            Some(Err(err)) => return err.to_compile_error().into(),
            None => None,
        };
        let subject = match bound_subject(
            authorize.as_ref(),
            &inputs,
            &method.sig.ident,
            &route_path,
            parsed_path.parameter_count(),
        ) {
            Ok(subject) => subject,
            Err(err) => return err.to_compile_error().into(),
        };
        if subject.is_some() {
            wrappers.push(bound_path_check(
                &self_ty,
                &route_path,
                parsed_path.parameter_count(),
                &cfgs,
            ));
            wrappers.push(unit_of_work_required(&self_ty, &cfgs));
        }
        if let Some(AuthorizeSpec {
            entity,
            bind: Some(service),
            ..
        }) = &authorize
        {
            wrappers.push(quote_spanned! {entity.span()=>
                #(#cfgs)*
                const _: () = ::nest_rs_seaorm::__private::bound_entity::<#service, #entity>();
            });
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
        // A shaper's status is the one the route sends: it is set after the answer.
        let success_status = response_shapers.success_status();
        let shaped_status = response_shapers.sets_status();
        let response_headers = response_shapers.headers.iter().map(|(name, value)| {
            quote! { (#name, #value) }
        });
        let response_headers = quote! { &[#(#response_headers),*] };
        let redirects = response_shapers.redirect.is_some();
        if redirects {
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
            // A fallible open is told by type through `nest_rs_core::__private::Answer`, its
            // error kept for `RouteReturn` to read by type. The return type is inferred: no
            // macro can name the handler's `impl Stream`.
            let wrapped = quote! {{
                use ::nest_rs_core::__private::AnswerFallback as _;
                let __nestrs_answer = #call_expr;
                ::nest_rs_core::__private::Answer(&__nestrs_answer).map()(
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
                quote! { ::nest_rs_http::Result<::nest_rs_http::Response> },
                body,
            )
        };

        if let Some(ty) = inputs.iter().find_map(|arg| match arg {
            FnArg::Typed(pt) if matches!(piped_parts(&pt.ty), Some((_, None))) => Some(&pt.ty),
            _ => None,
        }) {
            return syn::Error::new_spanned(ty, PIPED_NAMES_ITS_EXTRACTOR)
                .to_compile_error()
                .into();
        }
        // Each parameter read off the request in order, the body by at most one;
        // the controller `Arc` is a captured field.
        let extractor_stmts: Vec<TokenStream2> = inputs
            .iter()
            .enumerate()
            .filter_map(|(index, arg)| match arg {
                FnArg::Typed(pt) => {
                    let pat = &pt.pat;
                    let ty = &pt.ty;
                    Some(match &subject {
                        // The subject loads where the handler lists it, after the
                        // class gate the guard stage ran.
                        Some(bound) if bound.index == index => {
                            let BoundSubject {
                                action, service, ..
                            } = bound;
                            quote_spanned! {ty.span()=>
                                let #pat: #ty = ::nest_rs_seaorm::__private::http_bind::<
                                    #action,
                                    #service,
                                >(#req_var, &mut #body_var)
                                .await?;
                            }
                        }
                        // The rules the extracted value's type declares hold
                        // before a pipe runs over it and before the next
                        // parameter is read, the guards having run.
                        _ => match piped_parts(ty) {
                            Some((pipe, Some(source))) => {
                                let validated = validate_statement(&quote!(&__nestrs_source));
                                quote! {
                                    let #pat = {
                                        // Typed, so a source that is no extractor is
                                        // refused once, by its `FromRequest` bound.
                                        let __nestrs_source: #source =
                                            <#source as ::nest_rs_http::FromRequest>::from_request(
                                                #req_var, &mut #body_var,
                                            )
                                            .await?;
                                        #validated
                                        ::nest_rs_http::__private::piped::<#pipe, #source>(
                                            __nestrs_source,
                                        )?
                                    };
                                }
                            }
                            // Refused above, before any statement is built.
                            Some((_, None)) | None => {
                                let validated = validate_statement(&quote!(&#pat));
                                // Typed, so a parameter that is no extractor is
                                // refused once, by its `FromRequest` bound.
                                quote! {
                                    let #pat: #ty = <#ty as ::nest_rs_http::FromRequest>::from_request(
                                        #req_var, &mut #body_var,
                                    )
                                    .await?;
                                    #validated
                                }
                            }
                        },
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
            impl ::nest_rs_http::__private::RouteWrapper for #wrapper_name {
                #[allow(unused_mut)]
                async fn call(
                    &self,
                    #req_var: &mut ::nest_rs_http::Request,
                ) -> ::nest_rs_http::Result<::nest_rs_http::Response> {
                    let mut #body_var = ::nest_rs_http::RequestBody::new(#req_var.take_body());
                    let #req_var: &::nest_rs_http::Request = #req_var;
                    #(#extractor_stmts)*
                    let #ctrl_var = &self.__ctrl;
                    #sse_binding
                    let #res_var: #wrapper_return_type = async move { #wrapper_body }.await;
                    let #res_var = {
                        #[allow(unused_imports)]
                        use ::nest_rs_http::__private::RouteReturnFallback as _;
                        ::nest_rs_http::__private::RouteReturn(#res_var).into_result()
                    };
                    ::std::result::Result::map(
                        #res_var,
                        ::nest_rs_http::IntoResponse::into_response,
                    )
                }
            }
        });

        handler_consts.push(handler_const(&HandlerSite {
            unit: quote!(::nest_rs_http::unit::REQUEST),
            self_ty: &self_ty,
            host_name: &ctrl_name,
            method: &method_name,
            address: quote!(#route_path),
            posture: if is_public {
                SitePosture::Public
            } else {
                SitePosture::Gated
            },
            host_meta: HostMeta::Struct,
            method_meta: &metas,
            cfgs: &cfgs,
        }));

        // The compiler selects the arming type, so this crate needs no authz
        // dependency and a renamed import still arms.
        let mut param_types = param_types(&inputs);
        let probe_types: Vec<Type> = authorize_ty
            .iter()
            .chain(
                param_types
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !unprobed.contains(index))
                    .map(|(_, ty)| ty),
            )
            .cloned()
            .collect();
        let shaper_selection = shaper_selection(&probe_types);
        let typed_body = documented_body(&param_types);
        let typed_refusal = documented_refusal(&param_types);

        // Checked like a `Scoped<T>` parameter's: an unimported service fails
        // the boot.
        param_types.extend(subject.as_ref().map(|bound| -> Type {
            let service = &bound.service;
            parse_quote!(::nest_rs_core::Scoped<#service>)
        }));
        let handler = RouteHandler {
            method: method_const.clone(),
            deprecation_header: deprecation.as_ref().map(|(_, header)| header.clone()),
            is_sse,
            wrapper: wrapper_name.clone(),
            guards,
            interceptors,
            probe_types,
            param_types,
            posture_type: authorize_ty,
            stream_posture,
            no_pipes,
            force_guards,
            pipes: method_pipes,
            exception_filters: method_exception_filters,
            versions: method_versions.clone(),
            cfgs: cfgs.clone(),
            authorized: authorize.is_some(),
            method_name: method_name.clone(),
        };
        // Two handlers for one (verb, path) would leave one unreachable; refused
        // here without `#[cfg]`, and under both `#[cfg]`s when either has one.
        let served: Vec<String> = method_versions.iter().map(LitStr::value).collect();
        let route = match served.as_slice() {
            [] => format!("{method_const} {}", route_path.value()),
            versions => format!(
                "{method_const} {} #[version({})]",
                route_path.value(),
                versions
                    .iter()
                    .map(|version| format!("{version:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        };
        if let Err(err) = routes_declared.declare_in(
            "route",
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
            // Each parameter binds by the name the address was mounted under.
            Some(group) if group.mount.value() != route_path.value() => {
                return syn::Error::new_spanned(
                    &written_path,
                    format!(
                        "`#[routes]` mounts one address as `{}` on `{}` and as `{}` on `{}`: an \
                         address is mounted once and each parameter binds by its name, so one \
                         handler would read a parameter that is not there — name the parameters \
                         alike on both",
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

        let summary = opt_str(&api.summary);
        let description = opt_str(&api.description);
        let tags = if api.tags.is_empty() {
            quote! { &[#ctrl_token] }
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
        let mut request_refusal = quote! { ::core::option::Option::None };
        let request_body = match (&api.multipart, &json_body) {
            (Some(ty), _) => quote! {
                ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Multipart(
                    ::core::option::Option::Some(
                        ::nest_rs_http::__private::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
                    ),
                ))
            },
            (None, Some(ty)) => quote! {
                ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Json(
                    ::nest_rs_http::__private::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
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
                        ::nest_rs_http::__private::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
                    ))
                },
                None => {
                    request_refusal = typed_refusal;
                    typed_body
                }
            },
        };
        // Probed by its type for its body and what it answers beside it, so an
        // alias documents both; a type no expression can name, or an `#[sse]`
        // stream, is not probed.
        let answered = answer_type(&method.sig.output).filter(|_| !is_sse);
        // Kept under a posture naming an entity, recorded as `names_an_entity`:
        // its nullable properties read `null` where a grant withholds them.
        let response = match (&api.response, answered) {
            (Some(ty), _) => quote! {
                ::core::option::Option::Some(::nest_rs_http::__private::schema_of::<#ty> as ::nest_rs_http::SchemaFn)
            },
            (None, Some(ty)) => quote! { ::nest_rs_http::__payload_of!(#ty) },
            (None, None) => quote! { ::core::option::Option::None },
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
        let deprecation_meta = match &deprecation {
            Some((since, header)) => quote! {
                ::core::option::Option::Some(::nest_rs_http::__private::deprecation_meta(
                    #since,
                    #header,
                ))
            },
            None => quote! { ::core::option::Option::None },
        };
        let error_responses = api.errors.iter().map(|(status, ty)| {
            quote! { (#status, ::nest_rs_http::__private::schema_of::<#ty> as ::nest_rs_http::SchemaFn) }
        });
        let error_responses = quote! { &[#(#error_responses),*] };
        let response_content_type = match (
            api.response_content_type.clone(),
            response_shapers.content_type(),
            answered,
        ) {
            (Some(lit), ..) | (None, Some(lit), _) => {
                quote! { ::core::option::Option::Some(#lit) }
            }
            (None, None, _) if is_sse => {
                quote! { ::core::option::Option::Some("text/event-stream") }
            }
            (None, None, Some(ty)) => quote! { ::nest_rs_http::__media_of!(#ty) },
            (None, None, None) => quote! { ::core::option::Option::None },
        };
        let names_an_entity = quote! { ::core::option::Option::is_some(&#shaper_selection) };

        let path_param_tys = path_param_types(&inputs);
        let path_params = if path_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::__private::schema_of::<#path_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };
        let query_param_tys = extractor_payloads(&inputs, "Query");
        let query_params = if query_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::__private::schema_of::<#query_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };
        let header_param_tys = extractor_payloads(&inputs, "Header");
        let header_params = if header_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::__private::schema_of::<#header_param_tys> as ::nest_rs_http::SchemaFn),*] }
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
                    ::nest_rs_http::__private::versions_declare(<#self_ty>::VERSIONS, #route_versions),
                    #message,
                );
            });
        }

        let answer = match answered {
            Some(ty) => quote! { ::nest_rs_http::__answer_of!(#ty) },
            None => quote! { ::nest_rs_http::__private::AnswerDoc::OK },
        };
        let (success_status, answers_an_error) = if shaped_status {
            (
                quote! { ::core::option::Option::Some(#success_status) },
                quote! { false },
            )
        } else {
            (
                quote! { __nestrs_answer.status },
                quote! { __nestrs_answer.error },
            )
        };
        route_metas.push(quote! {
            #(#cfgs)*
            {
            let __nestrs_answer: ::nest_rs_http::__private::AnswerDoc = #answer;
            ::nest_rs_http::__private::http_route_meta(::nest_rs_http::__private::HttpRouteParts {
                method: ::nest_rs_http::Method::#method_const,
                path: #route_path,
                handler: #method_name_lit,
                summary: #summary,
                description: #description,
                tags: #tags,
                request_body: #request_body,
                request_refusal: #request_refusal,
                response: #response,
                response_content_type: #response_content_type,
                error_responses: #error_responses,
                names_an_entity: #names_an_entity,
                path_params: #path_params,
                query_params: #query_params,
                header_params: #header_params,
                may_conflict: #may_conflict,
                sets_location: #redirects || __nestrs_answer.location,
                sets_next_link: __nestrs_answer.next_link,
                answers_an_error: #answers_an_error,
                response_headers: #response_headers,
                success_status: #success_status,
                public: #is_public,
                deprecation: #deprecation_meta,
                versions: #route_versions,
                site: #site_name,
            })
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
                    .chain(&handler.interceptors)
                    .chain(&handler.pipes)
                    .chain(&handler.exception_filters)
                    .map(|item| Conditional {
                        cfgs: &handler.cfgs,
                        item,
                    })
            }),
    );
    let injected_methods = injected_methods_with_layers(&self_ty, &route_layers);
    // A provider a parameter resolves per request is checked like an `#[inject]`
    // one: an unimported `Scoped<T>` or bound service fails the boot.
    let injected_per_request = injected_per_request_method(
        routes_by_path
            .iter()
            .flat_map(|group| group.handlers.iter())
            .flat_map(|handler| {
                handler.param_types.iter().map(|item| Conditional {
                    cfgs: &handler.cfgs,
                    item,
                })
            }),
    );

    let capability_bounds = guard_capability_bounds(
        routes_by_path
            .iter()
            .flat_map(|group| group.handlers.iter())
            .flat_map(|handler| {
                handler.guards.iter().map(|item| Conditional {
                    cfgs: &handler.cfgs,
                    item,
                })
            }),
        quote!(::nest_rs_guards::GuardFor<::nest_rs_http::HttpView>),
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

    // Registered inside the per-version loop, one entry per verb that serves the
    // version: the table owns the method dispatch, `Allow` and `HEAD`.
    let mut route_entries: Vec<TokenStream2> = Vec::new();
    let mut sites: Vec<TokenStream2> = Vec::new();
    for group in &routes_by_path {
        let path = &group.mount;
        let mut arms: Vec<TokenStream2> = Vec::new();
        for handler in &group.handlers {
            let EmittedRoute { items, mount } = emitted_route(handler, &self_ty);
            sites.push(items);
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
            arms.push(quote! {
                #(#cfgs)*
                let () = if #serves {
                    #mount;
                };
            });
        }
        route_entries.push(quote! {
            {
                #[allow(unused_variables)]
                let __path = ::nest_rs_http::__private::join_path(&__prefix, #path);
                #(#arms)*
            }
        });
    }

    let crossings = routes_declared.crossings();
    let host_runtime = host_runtime_entry(
        &self_ty,
        &quote!(::nest_rs_http::__private::RUNTIME),
        HostMount::Attached,
        &[],
    );

    quote! {
        #item

        impl #self_ty {
            #(#handler_consts)*

            #(#sites)*
        }

        #crossings

        #capability_bounds

        #(#wrappers)*

        #host_runtime

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_methods

            #injected_per_request

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                let __meta = ::nest_rs_http::__private::http_controller_meta(
                    #ctrl_tag,
                    #ctrl_token,
                    <#self_ty>::PATH,
                    <#self_ty>::VERSIONS,
                    ::std::vec![#(#route_metas),*],
                    |container, __routes| {
                        ::nest_rs_http::__private::mount_controller(
                            container,
                            __routes,
                            #ctrl_tag,
                            &<#self_ty>::__nestrs_controller_guard_specs(),
                        );
                        let __ctrl =
                            ::std::sync::Arc::new(<#self_ty>::__nestrs_from_container(container));
                        #sse_resolve
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
                                ::nest_rs_http::__private::version_path(__version, <#self_ty>::PATH);
                            #(#route_entries)*
                        }
                    },
                );
                builder.attach_meta::<#self_ty, ::nest_rs_http::HttpControllerMeta>(__meta)
            }
        }
    }
    .into()
}

/// What `#[crud]` marks the parameters it generates with: a type that
/// declares neither a gate nor a shaper, so `#[routes]` probes it for neither.
pub(crate) const UNPROBED: &str = "__nestrs_unprobed";

/// A posture naming a subject: `#[authorize(Action, Subject)]`, or
/// `#[public(Action, Subject)]`.
struct AuthorizeSpec {
    action: Path,
    /// The subject written, or under `bind = Service` alone the one its
    /// `Authorized<Action, E>` parameter names.
    entity: Type,
    /// `bind = Service`: the service that loads the subject into the route's
    /// `Authorized<Action, E>` parameter.
    bind: Option<Path>,
}

/// The parameter `bind = Service` fills: where the handler lists it, and what
/// loads it.
struct BoundSubject {
    /// Its position among the handler's parameters, the receiver left out.
    index: usize,
    action: Path,
    service: Path,
}

/// The route's bound subject, refusing a `bind` with no `Authorized`
/// parameter to receive it, an `Authorized` parameter no `bind` fills, and a
/// route whose own path names more than the subject's id (`parameters` is how
/// many it declares); [`bound_path_check`] adds its controller's.
fn bound_subject(
    authorize: Option<&AuthorizeSpec>,
    inputs: &[FnArg],
    method: &syn::Ident,
    path: &LitStr,
    parameters: usize,
) -> syn::Result<Option<BoundSubject>> {
    let mut subjects = inputs
        .iter()
        .enumerate()
        .filter(|(_, arg)| spelled(arg, "Authorized"));
    let first = subjects.next();
    if let Some((_, second)) = subjects.next() {
        return Err(syn::Error::new_spanned(
            second,
            "a route binds one subject, so it takes one `Authorized<Action, E>` parameter",
        ));
    }
    let service = authorize.and_then(|spec| spec.bind.as_ref().map(|service| (spec, service)));
    match (service, first) {
        (None, None) => Ok(None),
        (None, Some((_, param))) => Err(syn::Error::new_spanned(param, AUTHORIZED_NEEDS_BIND)),
        (Some(_), None) => Err(syn::Error::new_spanned(method, BIND_NEEDS_AUTHORIZED)),
        (Some((spec, service)), Some((index, _))) => {
            if parameters > 1 {
                return Err(syn::Error::new_spanned(
                    path,
                    format!(
                        "{BOUND_PATH} and `{}` declares {parameters} — {BOUND_PATH_REMEDY}",
                        path.value(),
                    ),
                ));
            }
            Ok(Some(BoundSubject {
                index,
                action: spec.action.clone(),
                service: service.clone(),
            }))
        }
    }
}

/// What a bound route's path refusal opens with.
const BOUND_PATH: &str = "`bind = Service` loads the subject from the route's one path parameter,";

/// What a bound route's path refusal asks for.
const BOUND_PATH_REMEDY: &str = "bind on a route whose path, its controller's included, names \
                                 the subject's id alone, as `/{id}` or `/{id}/publish`";

/// The assertion that a bound route's whole path — its controller's, then its
/// own, which declares `parameters` — names one parameter: `#[routes]` never
/// sees the controller's path, so the sum is checked where both are known.
fn bound_path_check(
    self_ty: &Type,
    path: &LitStr,
    parameters: usize,
    cfgs: &[TokenStream2],
) -> TokenStream2 {
    let message = format!(
        "{BOUND_PATH} and `{}` under its `#[controller]` path does not name exactly one — \
         {BOUND_PATH_REMEDY}",
        path.value(),
    );
    // A literal message is a format string: its braces are the template's.
    let message = LitStr::new(&message.replace('{', "{{").replace('}', "}}"), path.span());
    quote_spanned! {path.span()=>
        #(#cfgs)*
        const _: () = ::core::assert!(
            <#self_ty>::__nestrs_controller_path_parameters() + #parameters == 1,
            #message,
        );
    }
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
    versioning::parse_version_args(&attr)
}

/// Read `#[word(Action, Subject)]`, or `#[word(Action, bind = Service)]`, off a
/// route method whose parameters, the receiver left out, are `inputs`.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn gate_of(attr: &Attribute, word: &'static str, inputs: &[FnArg]) -> syn::Result<AuthorizeSpec> {
    // Parsed as `Meta`, not `Path`, so `bind = Service` is refused by name rather
    // than by syn's `expected \`,\``.
    let metas: Vec<Meta> = attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|_| malformed_posture(attr, word))?
        .into_iter()
        .collect();
    let mut bind: Option<Path> = None;
    let mut paths: Vec<Path> = Vec::new();
    // A repeated key is refused, never last-write-wins: `bind` picks the loading service.
    let mut keys: Vec<syn::Ident> = Vec::new();
    for meta in metas {
        match meta {
            Meta::Path(path) => paths.push(path),
            Meta::NameValue(nv) if nv.path.is_ident("bind") => {
                keys.extend(nv.path.get_ident().cloned());
                let Expr::Path(service) = &nv.value else {
                    return Err(syn::Error::new_spanned(
                        &nv.value,
                        takes_value(word, Some("bind"), BIND_TAKES),
                    ));
                };
                bind = Some(service.path.clone());
            }
            Meta::NameValue(nv) if nv.path.is_ident("id_arg") => {
                return Err(syn::Error::new_spanned(
                    &nv,
                    posture_key_unsupported(
                        "id_arg = argument",
                        "HTTP",
                        ID_ARG_UNSUPPORTED_BECAUSE,
                    ),
                ));
            }
            Meta::NameValue(ref nv) if let Some(key) = nv.path.get_ident() => {
                return Err(syn::Error::new_spanned(
                    key,
                    unknown_argument(word, &key.to_string(), POSTURE_KEYS),
                ));
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    malformed_posture(attr, word),
                ));
            }
        }
    }
    nest_rs_codegen::Grammar::new(word, POSTURE_KEYS).take_all(&keys)?;
    if let Some(unmasked) = paths.iter().find(|p| p.is_ident("unmasked")) {
        return Err(syn::Error::new_spanned(
            unmasked,
            nest_rs_codegen::__private::unmasked_refused(),
        ));
    }
    let mut paths = paths.into_iter();
    let (Some(action), written, None) = (paths.next(), paths.next(), paths.next()) else {
        return Err(malformed_posture(attr, word));
    };
    let entity: Type = match (written, &bind) {
        (Some(entity), _) => parse_quote!(#entity),
        // The `E` of the `Authorized<Action, E>` parameter, asserted the
        // service's once (`bound_entity`) rather than projected in every probe;
        // a route with none is refused by `bound_subject`.
        (None, Some(service)) => authorized_entity(inputs)
            .cloned()
            .unwrap_or_else(|| parse_quote!(<#service as ::nest_rs_seaorm::CrudService>::Entity)),
        (None, None) => return Err(malformed_posture(attr, word)),
    };
    Ok(AuthorizeSpec {
        action,
        entity,
        bind,
    })
}

/// What an `#[sse]` route whose posture names an entity is told.
const SSE_NAMES_AN_ENTITY: &str = "an `#[sse]` route's posture names a subject no table holds: an \
     entity's posture opens reads whose rows the stream writes after the route's unit ended, \
     where no ability is left to judge them. Declare the capability with `#[subject]` on a unit struct and gate the stream with \
     `#[authorize(Action, ThatSubject)]` or `#[public(Action, ThatSubject)]`, then load what it \
     needs inside the handler";

/// The keys a route's posture takes beside its action and subject.
const POSTURE_KEYS: &[&str] = &["bind"];

/// The posture's shape refusal, in the words of the attribute `word` names.
fn malformed_posture(attr: &Attribute, word: &str) -> syn::Error {
    syn::Error::new_spanned(
        attr,
        format!(
            "expected `#[{word}(Action, Subject)]` — e.g. `#[{word}(Read, users::Entity)]`. `bind \
             = Service` loads the subject from the route's path parameter into an \
             `Authorized<Action, E>` parameter, and lets the entity be omitted (derived from \
             `Service::Entity`): `#[{word}(Update, bind = PostsService)]`"
        ),
    )
}

/// The type a posture naming a subject declares, the same one `#[crud]` writes:
/// probed for its class gate and its shaper, never extracted. Under
/// `bind = Service` alone, the entity is the `Authorized<Action, E>`
/// parameter's.
fn posture_type(spec: &AuthorizeSpec) -> Type {
    let AuthorizeSpec { action, entity, .. } = spec;
    syn::parse_quote! {
        ::nest_rs_authz::http::Authorize<#action, #entity>
    }
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

/// The first answer `probe!` gives over `param_types`, of type `Option<answer>`,
/// selected by type after name resolution.
fn first_probed(param_types: &[Type], answer: &TokenStream2, probe: &TokenStream2) -> TokenStream2 {
    if param_types.is_empty() {
        return quote! { ::core::option::Option::<#answer>::None };
    }
    quote! {{
        let mut __nestrs_first: ::core::option::Option<#answer> = ::core::option::Option::None;
        #(
            if __nestrs_first.is_none() {
                __nestrs_first = #probe!(#param_types);
            }
        )*
        __nestrs_first
    }}
}

/// The body an extractor that is the body itself documents, selected like the
/// shaper: the first parameter that is a `DocumentedBody`, which no spelling names.
fn documented_body(param_types: &[Type]) -> TokenStream2 {
    first_probed(
        param_types,
        &quote! { ::nest_rs_http::RequestBodyMeta },
        &quote! { ::nest_rs_http::__body_of },
    )
}

/// What the first `DocumentedBody` parameter refuses in, when its own words.
fn documented_refusal(param_types: &[Type]) -> TokenStream2 {
    first_probed(
        param_types,
        &quote! { ::nest_rs_http::SchemaFn },
        &quote! { ::nest_rs_http::__refusal_of },
    )
}

/// The route's class gates, selected by type after name resolution like its
/// shaper: every probed type whose `RouteResponseShaper` declares one.
fn gate_selection(probe_types: &[Type]) -> TokenStream2 {
    if probe_types.is_empty() {
        return quote! { ::std::vec::Vec::new() };
    }
    quote! {{
        let mut __nestrs_gates: ::std::vec::Vec<::nest_rs_http::__private::GateFn> =
            ::std::vec::Vec::new();
        #(
            if let ::core::option::Option::Some(__nestrs_gate) =
                ::nest_rs_http::__gate_of!(#probe_types)
            {
                __nestrs_gates.push(__nestrs_gate);
            }
        )*
        __nestrs_gates
    }}
}

/// The route's response shaper, selected by type after name resolution: the
/// first parameter that is a `RouteResponseShaper` arms the route, so an
/// aliased import arms too.
fn shaper_selection(param_types: &[Type]) -> TokenStream2 {
    first_probed(
        param_types,
        &quote! { ::nest_rs_http::__private::CaptureFn },
        &quote! { ::nest_rs_http::__shaper_of },
    )
}

/// One routed handler, as its route's site declares it and its mount mounts
/// it: the site (its interceptors and exception filters, its guard stage and
/// class gates), and the call registering the handler — its response shaper
/// innermost, its global pipes before it — on the pipeline.
struct EmittedRoute {
    /// The controller's hidden associated items: the site, its layers and its
    /// guard stage.
    items: TokenStream2,
    /// Inside the controller's mount closure, where `container`, `__routes`,
    /// `__path`, `__ctrl` and `__sse` are in scope.
    mount: TokenStream2,
}

fn emitted_route(handler: &RouteHandler, self_ty: &Type) -> EmittedRoute {
    let RouteHandler {
        method,
        param_types,
        deprecation_header,
        versions: _,
        cfgs,
        is_sse,
        wrapper,
        guards,
        interceptors,
        probe_types,
        posture_type,
        stream_posture,
        no_pipes,
        force_guards,
        pipes: method_pipes,
        exception_filters: method_exception_filters,
        authorized,
        method_name,
    } = handler;

    let shaper_selection = shaper_selection(probe_types);
    let gate_selection = gate_selection(probe_types);
    let posture_assert = match posture_type {
        Some(ty) => quote! {
            const _: fn() = || {
                fn __nestrs_assert_route_shaper<P: ::nest_rs_http::RouteResponseShaper>() {}
                __nestrs_assert_route_shaper::<#ty>();
            };
        },
        None => quote! {},
    };
    let method_guard_specs = scoped_specs(guards, quote!(dyn ::nest_rs_guards::Guard));
    let ambient = ambient_type_ids(param_types);
    let force_guard_typeids = force_guard_typeids(force_guards);
    let method_pipe_specs = scoped_specs(method_pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let no_pipes_flag = if *no_pipes {
        quote!(true)
    } else {
        quote!(false)
    };
    let items = site_items(&SiteItems {
        self_ty,
        method: method_name,
        cfgs,
        host_interceptors: quote!(<#self_ty>::__nestrs_controller_interceptor_specs()),
        host_exception_filters: quote!(<#self_ty>::__nestrs_controller_exception_filter_specs()),
        interceptors,
        exception_filters: method_exception_filters,
        global: quote!(::nest_rs_core::__private::GlobalLayers::ALL),
        guards: Some(SiteGuards {
            build: quote!(::nest_rs_http::__private::route_guard_stage),
            declaration: quote! {
                ::nest_rs_http::__private::RouteGuards {
                    host: <#self_ty>::__nestrs_controller_guard_specs(),
                    method: #method_guard_specs,
                    force: #force_guard_typeids,
                    gates: #gate_selection,
                    authorized: #authorized,
                    names_an_entity: ::core::option::Option::is_some(&#shaper_selection),
                    stream: #is_sse,
                    ambient: #ambient,
                }
            },
        }),
        class_gate: None,
    });
    let site = site_ref(self_ty, method_name);

    // Expanded inside the mount closure, where `__ctrl` and `__sse` are in scope.
    let wrapper_expr = if *is_sse {
        quote! { #wrapper { __ctrl: ::std::sync::Arc::clone(&__ctrl), __sse: __sse } }
    } else {
        quote! { #wrapper { __ctrl: ::std::sync::Arc::clone(&__ctrl) } }
    };
    let deprecated = match deprecation_header {
        Some(header) => quote! { ::core::option::Option::Some(#header) },
        None => quote! { ::core::option::Option::None },
    };
    let mount = quote! {
        ::nest_rs_http::__private::mount_route(
            container,
            __routes,
            ::nest_rs_http::Method::#method,
            &__path,
            #site,
            ::nest_rs_http::__private::RoutePipes {
                host: <#self_ty>::__nestrs_controller_pipe_specs(),
                method: #method_pipe_specs,
                none: #no_pipes_flag,
            },
            {
                #posture_assert
                #stream_posture
                ::nest_rs_http::__private::shaped(#wrapper_expr, #shaper_selection)
            },
            #deprecated,
        )
    };
    EmittedRoute { items, mount }
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
            takes_value(
                &verb,
                None,
                &format!("the route's path as a string literal, e.g. `#[{verb}(\"/{{id}}\")]`"),
            ),
        )
    };
    let written: Expr = attr.parse_args().map_err(|_| refused(attr))?;
    match ungrouped_expr(&written) {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(path),
            ..
        }) => Ok(path.clone()),
        other => Err(refused(other)),
    }
}

#[derive(Default)]
struct ApiMeta {
    summary: Option<LitStr>,
    description: Option<LitStr>,
    tags: Vec<LitStr>,
    /// `#[api(response = T)]` — the payload the document advertises when the
    /// return type names none (a built `Response`); it wins over the probe.
    response: Option<Type>,
    /// `#[api(multipart = T)]` — the type describing the parts of a
    /// `multipart/form-data` body.
    multipart: Option<Type>,
    /// `#[api(response_content_type = "audio/mpeg")]` — the media type of a
    /// success body that is not JSON.
    response_content_type: Option<LitStr>,
    /// `#[api(error(503 = ProbeReport))]` — an error status the handler answers
    /// with a JSON body of its own, and that body's type.
    errors: Vec<(syn::LitInt, Type)>,
    /// `#[api(deprecated = "2026-10-08")]` — the date the route was deprecated on.
    deprecated: Option<LitStr>,
}

/// Every key `#[api]` takes. A key the match in [`parse_api_attr`] does not
/// read is accepted and dropped: `every_api_key_is_read_as_the_table_writes_it`
/// holds the two together.
const API_KEYS: [&str; 8] = [
    "summary",
    "description",
    "tags",
    "response",
    "multipart",
    "response_content_type",
    "error",
    "deprecated",
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
            "error" => out.errors.extend(api_errors(arg.input(), arg.ident())?),
            "deprecated" => out.deprecated = Some(arg.str_lit("2026-10-08")?),
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
            takes_value(
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
            takes_value(
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
        .map(|tag| match ungrouped_expr(tag) {
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(tag),
                ..
            }) => Ok(tag.clone()),
            other => Err(refused(other)),
        })
        .collect()
}

/// `error(503 = ProbeReport, 409 = ConflictDto)` — error statuses, each with
/// the type of the JSON body the handler answers it with.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the key accepts; syn's own message would name a token"
)]
fn api_errors(
    input: syn::parse::ParseStream<'_>,
    key: &syn::Ident,
) -> syn::Result<Vec<(syn::LitInt, Type)>> {
    const TAKES: &str = "a list of `status = Type`, the status a 4xx or 5xx, e.g. \
                         `error(503 = ProbeReport)`; the success payload is `response = T`";
    let refused =
        |at: &dyn ToTokens| syn::Error::new_spanned(at, takes_value("api", Some("error"), TAKES));
    if !input.peek(syn::token::Paren) {
        return Err(refused(key));
    }
    let content;
    syn::parenthesized!(content in input);
    let mut out: Vec<(syn::LitInt, Type)> = Vec::new();
    while !content.is_empty() {
        let status: syn::LitInt = content.parse().map_err(|_| refused(key))?;
        let code: u16 = status.base10_parse().map_err(|_| refused(&status))?;
        if !(400..=599).contains(&code) {
            return Err(refused(&status));
        }
        if out
            .iter()
            .any(|(seen, _)| seen.base10_parse::<u16>().ok() == Some(code))
        {
            return Err(syn::Error::new_spanned(
                &status,
                format!(
                    "{}: {code} is declared twice — one status, one body",
                    site("api", Some("error")),
                ),
            ));
        }
        content.parse::<Token![=]>().map_err(|_| refused(&status))?;
        let ty: Type = content.parse().map_err(|_| refused(&status))?;
        out.push((status, ty));
        if content.is_empty() {
            break;
        }
        content.parse::<Token![,]>()?;
    }
    if out.is_empty() {
        return Err(refused(key));
    }
    Ok(out)
}

/// A route's pipe reads what an extractor carries, which the carrier names.
const PIPED_NAMES_ITS_EXTRACTOR: &str = "a pipe on a route names the extractor it reads — write \
     `Piped<P, Json<T>>` (or `Query<T>`, `Form<T>`, `Path<T>`, `Header<T>`); `Piped<P>` is the \
     form of the edges that decode their argument themselves";

/// Hold an extracted argument, `argument` borrowing it, to the rules its type
/// declares, a refusal answered as a pipe's `400`.
fn validate_statement(argument: &TokenStream2) -> TokenStream2 {
    let check = validate_argument(&quote!(::nest_rs_http), argument);
    quote! {
        #check.map_err(|__refused| ::nest_rs_http::pipe_error(&__refused))?;
    }
}

/// A `Piped<P, S>` parameter, read off its spelling: its pipe, and the extractor
/// it reads when it names one. `Piped` is no extractor, so one behind an alias
/// does not compile.
fn piped_parts(ty: &Type) -> Option<(&Type, Option<&Type>)> {
    let pipe = nth_generic_type(ty, "Piped", 0)?;
    Some((pipe, nth_generic_type(ty, "Piped", 1)))
}

/// The payload type behind an extractor named `name`: `Name<T>` and
/// `Piped<_, Name<T>>` both yield `T`.
fn extractor_payload(ty: &Type, name: &str) -> Option<Type> {
    if let Some(payload) = nth_generic_type(ty, name, 0) {
        return Some(payload.clone());
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
/// yields `[A, B]`, as `Path` binds tuple elements left to right.
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

/// Whether the handler pulls the parts itself through a `Multipart` parameter.
fn takes_multipart(inputs: &[FnArg]) -> bool {
    inputs.iter().any(|arg| match arg {
        FnArg::Typed(pt) => last_segment_is(&pt.ty, "Multipart"),
        FnArg::Receiver(_) => false,
    })
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
                site("api", Some("response_content_type")),
            ),
        ));
    }
    Ok(())
}

/// The return type an expression in the controller's `register` can name, for
/// the answer and payload probes: none naming a lifetime but `'static` or `'_`
/// (which the expression infers, as it does an elided one), hiding behind
/// `impl`, or leaving a part to inference.
fn answer_type(output: &ReturnType) -> Option<&Type> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    fn nameable(tokens: TokenStream2) -> bool {
        let mut tokens = tokens.into_iter();
        while let Some(token) = tokens.next() {
            let named = match token {
                proc_macro2::TokenTree::Group(group) => nameable(group.stream()),
                proc_macro2::TokenTree::Ident(ident) => ident != "impl" && ident != "_",
                proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '\'' => matches!(
                    tokens.next(),
                    Some(proc_macro2::TokenTree::Ident(lifetime))
                        if lifetime == "static" || lifetime == "_"
                ),
                proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => true,
            };
            if !named {
                return false;
            }
        }
        true
    }
    nameable(quote!(#ty)).then_some(&**ty)
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    /// How each of [`API_KEYS`] is written, position for position.
    const API_KEYS_WRITTEN: [&str; 8] = [
        "summary = \"...\"",
        "description = \"...\"",
        "tags(\"a\", \"b\")",
        "response = Type",
        "multipart = Type",
        "response_content_type = \"type/subtype\"",
        "error(503 = Type)",
        "deprecated = \"2026-10-08\"",
    ];

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
    fn api_error_reads_each_status_and_its_body() {
        let meta = match api_attr(quote! { error(503 = ProbeReport, 409 = crate::Conflict) }) {
            Ok(meta) => meta,
            Err(err) => panic!("the argument list must parse: {err}"),
        };
        let read: Vec<(u16, String)> = meta
            .errors
            .iter()
            .map(|(status, ty)| {
                (
                    status.base10_parse().expect("a status"),
                    quote!(#ty).to_string(),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                (503, quote!(ProbeReport).to_string()),
                (409, quote!(crate::Conflict).to_string()),
            ],
        );
    }

    #[test]
    fn api_error_refuses_a_status_that_is_not_an_error_or_said_twice() {
        for bad in [
            quote! { error(200 = Report) },
            quote! { error(302 = Report) },
            quote! { error(Report) },
            quote! { error = Report },
            quote! { error() },
        ] {
            let msg = match api_attr(bad.clone()) {
                Ok(_) => panic!("`{bad}` must be refused"),
                Err(err) => err.to_string(),
            };
            assert!(msg.contains("4xx or 5xx"), "{msg}");
        }
        let twice = match api_attr(quote! { error(503 = A, 503 = B) }) {
            Ok(_) => panic!("a status declared twice must be refused"),
            Err(err) => err.to_string(),
        };
        assert!(twice.contains("declared twice"), "{twice}");
    }

    #[test]
    fn a_media_type_with_a_parameter_is_accepted() {
        api_attr(quote! { response_content_type = "text/event-stream; charset=utf-8" })
            .expect("a parameterized media type is still a media type");
    }

    #[test]
    fn a_payload_is_unwrapped_through_the_pipe_carriers() {
        for name in ["Header", "Json", "Form", "Query", "Path"] {
            let name_ident = format_ident!("{name}");
            let shapes: [Type; 2] = [
                parse_quote!(#name_ident<Tracing>),
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
    fn a_return_type_is_probed_when_an_expression_can_name_it() {
        let probed = |output: ReturnType| answer_type(&output).is_some();
        assert!(probed(parse_quote!(-> Result<Created<Json<Post>>>)));
        assert!(probed(parse_quote!(-> Json<&'static str>)));
        assert!(probed(parse_quote!(-> Json<Cow<'_, str>>)));
        assert!(probed(parse_quote!(-> Json<&str>)));
        assert!(probed(
            parse_quote!(-> Result<Json<u8>, Box<dyn Error + 'static>>)
        ));
        assert!(!probed(parse_quote!(-> Json<Cow<'a, str>>)));
        assert!(!probed(parse_quote!(-> impl IntoResponse)));
        assert!(!probed(parse_quote!(-> Json<Vec<_>>)));
        assert!(!probed(ReturnType::Default));
    }

    #[test]
    fn a_bare_multipart_parameter_is_detected_however_it_is_spelled() {
        let inputs: Vec<FnArg> = vec![
            parse_quote!(query: Query<TranscodeDto>),
            parse_quote!(form: nest_rs::http::Multipart),
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
                        `tags`, `response`, `multipart`, `response_content_type`, `error` or `deprecated`";
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
