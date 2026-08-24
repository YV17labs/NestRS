//! `#[routes]` — bind a `#[controller]` impl block's verb-tagged methods to
//! HTTP routes; emit `Controller` mount + `Discoverable`; capture per-route
//! OpenAPI metadata.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, ImplItem, LitStr, Meta, Path, ReturnType, Token, Type, parse_quote,
};

use nest_rs_codegen::{
    force_guard_typeids, guard_capability_bounds, impl_self_ident, injected_methods_with_layers,
    layer_deps, mixed_site_ident, normalize_forwarded_args, nth_generic_type, require_str_lit,
    scoped_specs, take_flag_attr, take_path_list,
};

use crate::attr::opt_str;

/// One route handler, by named field — a positional tuple here once let a
/// field-order slip silently swap e.g. `force_guards`/`pipes`.
struct RouteHandler {
    /// The HTTP verb ident (`get`, `post`, …).
    verb: syn::Ident,
    /// The generated wrapper fn's ident.
    wrapper: syn::Ident,
    /// Whether the verb was `#[sse]` — the endpoint carries the resolved
    /// [`SseSettings`] and wraps the handler's stream.
    is_sse: bool,
    /// `#[use_guards]` paths on the method.
    guards: Vec<Path>,
    /// `#[use_filters]` paths on the method.
    filters: Vec<Path>,
    /// `#[use_interceptors]` paths on the method.
    interceptors: Vec<Path>,
    /// Every declared parameter type, in order — the input the compiler
    /// answers "is this a response shaper?" for. Arming is type-directed, so
    /// this list, not a name, is what decides.
    param_types: Vec<Type>,
    /// A parameter whose type is *spelled* `Authorize<..>` / `Bind<..>`, kept
    /// only for the eager HTTP-D1 diagnostic — a type wearing that name and
    /// not implementing the shaper trait is a spanned compile error rather
    /// than a route that quietly arms nothing.
    named_shaper: Option<Type>,
    /// Whether the handler declares any extractor parameter. `false` proves
    /// no masking extractor can ever run — the run-time mask probe is dead
    /// weight and is not emitted.
    has_extractors: bool,
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
    /// `#[version("2")]` on the method — the subset of the controller's
    /// versions this route serves. Empty means all of them.
    versions: Vec<LitStr>,
}

/// Handlers grouped by path in first-seen order. Several verbs may share a
/// path (`GET` + `POST /users`), and poem rejects two `.at(path, ..)` for the
/// same path, so they must collapse into one method table
/// (`MethodTable::new().get(h1).post(h2)`), which is also what carries the verb
/// set to the `Allow` header a `405` owes.
type RoutesByPath = Vec<(LitStr, Vec<RouteHandler>)>;

pub(crate) fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    // The impl half collects; it declares nothing. Taking an argument list and
    // dropping it is the defect `#[processor]` and `#[scheduled]` were fixed
    // for, and this is the likeliest place of all to reach for `version` —
    // `#[controller]`, one line up, does declare one.
    if let Err(err) = crate::controller::HTTP_PAIR.reject_args(
        &TokenStream2::from(args),
        "a controller's `path` and `version` are declared by",
    ) {
        return err.to_compile_error().into();
    }
    let mut item = match crate::controller::HTTP_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    // Silent here until now: `use_guards` is no standalone attribute macro, so
    // it reached rustc as `cannot find attribute` with no transport, reason or
    // remedy named.
    if let Err(err) = crate::controller::HTTP_PAIR.reject_host_layers(&item.attrs) {
        return err.to_compile_error().into();
    }
    let self_ty = item.self_ty.clone();

    // Default OpenAPI tag — routes group by controller unless `#[api(tags(...))]` overrides.
    let ctrl_name = match impl_self_ident(&self_ty, "#[routes]") {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };
    let ctrl_tag = LitStr::new(&ctrl_name.to_string(), ctrl_name.span());
    // The controller half of an OpenAPI `operationId`, computed here because
    // this is where the type name is: a runtime crate cannot reach
    // `nest_rs_codegen` without dragging `syn` into every app's dependency
    // graph, and the document is rebuilt on every `/api-json` request.
    let ctrl_token = LitStr::new(&controller_token(&ctrl_name.to_string()), ctrl_name.span());

    let mut wrappers: Vec<TokenStream2> = Vec::new();
    let mut routes_by_path: RoutesByPath = Vec::new();
    let mut route_metas: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let verb_idx = method.attrs.iter().position(|attr| {
            ["get", "post", "put", "delete", "patch", "sse"]
                .iter()
                .any(|v| attr.path().is_ident(v))
        });
        let Some(idx) = verb_idx else { continue };

        let attr = method.attrs.remove(idx);
        let declared_verb = attr
            .path()
            .get_ident()
            .expect("verb attribute has an ident")
            .clone();
        // `#[sse]` is a `GET` that answers `text/event-stream` — a response
        // shape, not a sixth method. Collapsing it here is what makes
        // `#[sse("/x")]` beside `#[get("/x")]` the same duplicate-route error
        // any two verbs get, instead of two handlers racing for one address.
        let is_sse = declared_verb == "sse";
        let verb_ident = if is_sse {
            format_ident!("get", span = declared_verb.span())
        } else {
            declared_verb.clone()
        };

        let route_path: LitStr = match attr.parse_args() {
            Ok(p) => p,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_name = method.sig.ident.clone();
        let method_name_lit = method_name.to_string();
        // Qualified by the **controller**, not the method alone. Each verb
        // becomes a module-level type (poem's `#[handler]` shape), so two
        // controllers in one file — the documented layout for URI versioning,
        // where `V1Controller::list` and `V2Controller::list` sit side by
        // side — collided in a namespace neither knew it shared. `list` /
        // `get` / `create` are exactly the names that repeat.
        let wrapper_name = format_ident!("__nestrs_route_{}_{}", ctrl_name, method_name);

        let mut inputs: Vec<FnArg> = method.sig.inputs.iter().skip(1).cloned().collect();
        // The wrapper declares the developer's arguments under plain names it can
        // forward by, so a destructured `Path(name): Path<String>` — poem's own
        // idiom — becomes `name: Path<String>` here and the developer's method
        // keeps its pattern. Done before the `#[authorize]` insert below, which
        // adds a wrapper-only parameter that is never forwarded.
        let arg_idents = match normalize_forwarded_args(&mut inputs) {
            Ok(idents) => idents,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[authorize(Action, Entity)]` — the route's posture, uniform with
        // `#[resolver]`. Desugars to the shaper extractor the macro writes
        // itself, so the arming can no longer be broken by how the developer
        // spelled (or renamed) an import.
        let authorize = match take_authorize(&mut method.attrs) {
            Ok(spec) => spec,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[authorize]` arms response masking, which round-trips the body
        // through the entity model. A stream of events is not a wire model and
        // never will be — there is no value to reconcile, and the shaper would
        // fail closed at 500 on every request. This is the same case as a
        // presigned URL or a computed report, and it has the same answer.
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
        // The decorator is not the only way to reach the shaper: `#[public]` +
        // a hand-written `Authorize<A, E>` is the sanctioned "public reads"
        // spelling, and `Bind<A, S>` arms it too. On a stream the shaper is
        // worse than useless — the mask classifies `text/event-stream` as an
        // opaque body and returns it untouched, so the route reads as masked,
        // documents itself as masked, and masks nothing. Refuse the parameter,
        // not just the attribute.
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
        // Captured before `guards` is moved into the handler tuple — feeds the
        // route's `scoped_guarded` flag (combined at runtime with any
        // controller-level guards) for the boot-time posture check.
        let method_guarded = !guards.is_empty();
        // Likewise for the `throttled` flag (OAPI-O4): a method-level
        // `ThrottlerGuard` here, or a controller-level one via the runtime call
        // emitted below.
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
        // `#[public]` marks the route as publicly reachable; global guards still
        // run and decide whether to admit anonymous callers.
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
        // `#[version("2")]` narrows this route to a subset of the controller's
        // versions — the "v2 adds one endpoint" case, which otherwise costs a
        // whole second controller.
        let method_versions = match take_version_attr(&mut method.attrs) {
            Ok(versions) => versions,
            Err(err) => return err.to_compile_error().into(),
        };
        // `#[no_pipes]` opts out of every global pipe for this route.
        let no_pipes = match take_flag_attr(&mut method.attrs, "no_pipes") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };
        // Internal marker the `#[crud]` macro stamps on its write ops (create /
        // update / delete) — their write-error mapper can surface a `409` on a
        // uniqueness violation, so the document advertises that response. Always
        // stripped here so it never reaches the compiler.
        let may_conflict = match take_flag_attr(&mut method.attrs, "crud_write") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };
        // The same kind of marker, for the `Location` a `#[crud]` create sends
        // with its `201`. Stamped by the generated handler because only it knows
        // it built the header; `#[redirect]` is added to it below, since a
        // redirect's whole response *is* a `Location`.
        let mut sets_location = match take_flag_attr(&mut method.attrs, "crud_location") {
            Ok(flag) => flag,
            Err(err) => return err.to_compile_error().into(),
        };

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
        // A `#[sse]` route answers `200 text/event-stream` and keeps answering
        // it: there is no status to override, no redirect to send instead of a
        // stream, and a header shaper would have to run before the first event
        // rather than around a body that never completes. One sentence for the
        // three, so a fourth response decorator inherits the refusal rather
        // than needing its own.
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
        // The effective success status the OpenAPI document advertises for this
        // route (OAPI-O3): a `#[redirect]`/`#[http_code(N)]` overrides the 200
        // default.
        let success_status = response_shapers.success_status();
        if response_shapers.redirect.is_some() {
            // A redirect *is* a `Location` — the shaper builds the response
            // around that one header — so the document declares it without a
            // marker of its own.
            sets_location = true;
            // And `#[redirect]` produces the response itself, never calling the
            // method, so rustc sees a method nothing invokes and reports
            // `dead_code` on code that is correct and documented. The macro
            // knows better than the lint here, so it says so — leaving a clean
            // build clean.
            method.attrs.push(parse_quote! {
                #[allow(dead_code, reason = "#[redirect] answers without calling the handler")]
            });
        }

        // Every local the wrapper binds for itself sits on `Span::mixed_site()`
        // — the span that gives *definition-site* hygiene to local variables —
        // so a handler parameter spelled `req`, `body`, `__ctrl` or `res` binds
        // a genuinely different variable and cannot shadow the wrapper's own.
        // Without it, `Json(body): Json<T>` — the idiom `/http/extractors/`
        // teaches — masked the `RequestBody` every *later* extractor reads, and
        // the mismatched-type error was reported on `#[routes]`, naming neither
        // the parameter nor the collision (HTTP-M1). The response shapers bind
        // three more (`response.rs`); they take the same span, so the guarantee
        // does not rest on statement order.
        let req_var = mixed_site_ident("req");
        let body_var = mixed_site_ident("body");
        let ctrl_var = mixed_site_ident("__ctrl");
        let res_var = mixed_site_ident("res");

        let call_expr = quote! { #ctrl_var.#method_name(#(#arg_idents),*).await };
        let returns_result = match &method.sig.output {
            ReturnType::Type(_, ty) => result_inner(ty).is_some(),
            ReturnType::Default => false,
        };
        let (wrapper_return_type, wrapper_body) = if is_sse {
            // The handler hands back a stream of `SseEvent`; the decorator owns
            // turning it into the response, applying the keep-alive and arming
            // the connection ceiling — none of which the developer should have
            // to remember, and the ceiling least of all. `Result::map` covers
            // the fallible open (`-> Result<impl Stream<…>, E>`) without naming
            // `E`; a bare stream takes the direct call. The return type is left
            // to inference: `SSE` and `Result<SSE, E>` are both `IntoResponse`,
            // and spelling either would mean naming the handler's own `impl
            // Stream` opaque type, which no macro can.
            let wrapped = if returns_result {
                quote! {
                    ::core::result::Result::map(#call_expr, |__nestrs_stream| {
                        ::nest_rs_http::SseSettings::respond(&__nestrs_sse, __nestrs_stream)
                    })
                }
            } else {
                quote! {
                    ::nest_rs_http::SseSettings::respond(&__nestrs_sse, #call_expr)
                }
            };
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
                returns_result,
            );
            (
                quote! { ::nest_rs_http::poem::Result<::nest_rs_http::poem::Response> },
                body,
            )
        };

        // Mirrors poem's own `#[handler]` expansion (split → extract each
        // parameter → call → `IntoResult` → `IntoResponse`), with one
        // deliberate difference: the controller `Arc` is a **captured field**
        // instead of a `Data<&Arc<Self>>` extractor. The `Data` route cost a
        // per-request extension insert (`.data` middleware) plus the anymap
        // it allocates — pure overhead on every request for a value that is
        // fixed at mount time.
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
        // The SSE settings are read from `HttpConfig` **once, at mount** and
        // carried in the endpoint beside the controller `Arc` — they are fixed
        // for the life of the process, and a container lookup per request on a
        // route whose whole job is to stay open would be pure overhead. Copied
        // out of `self` before the async block so the future captures a value
        // rather than borrowing the endpoint.
        let (sse_field, sse_binding) = if is_sse {
            (
                quote! { __sse: ::nest_rs_http::SseSettings, },
                quote! { let __nestrs_sse = self.__sse; },
            )
        } else {
            (quote! {}, quote! {})
        };
        wrappers.push(quote! {
            #[allow(non_camel_case_types)]
            struct #wrapper_name {
                __ctrl: ::std::sync::Arc<#self_ty>,
                #sse_field
            }

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

        // Which parameter arms the shaper is the compiler's answer, not this
        // crate's: it collects the types and emits the selection, which keeps
        // it free of any dep on the authz crate *and* immune to a rename.
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

        // The body, as one value pairing its media type with its schema — a
        // `Json<T>` extractor, `#[api(multipart = T)]` naming the parts of a
        // form, or a bare `Multipart` parameter whose parts no type states.
        let json_body = first_extractor_payload(&inputs, "Json");
        let form_body = first_extractor_payload(&inputs, "Form");
        // A route has one request body, so the three ways to declare one are
        // mutually exclusive. Asked as one table rather than as a check per
        // pair: the pairs grow quadratically with the ways, and the third way
        // (`Form<T>`) arrived without the two checks it owed.
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
            // A handler pulling the parts itself still declares the media type
            // it accepts: silence would document no body at all.
            (None, None) if takes_multipart(&inputs) => quote! {
                ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Multipart(
                    ::core::option::Option::None,
                ))
            },
            // `Form<T>` is a body like the other two, and matching only `Json`
            // and `Multipart` documented a form-encoded route as taking none —
            // silently, since nothing refused the shape either.
            (None, None) => match &form_body {
                Some(ty) => quote! {
                    ::core::option::Option::Some(::nest_rs_http::RequestBodyMeta::Form(
                        ::nest_rs_http::schema_of::<#ty> as ::nest_rs_http::SchemaFn,
                    ))
                },
                None => quote! { ::core::option::Option::None },
            },
        };
        // The payload the document advertises: `#[api(response = T)]` when the
        // handler states it, else the `Json<T>` the return type carries.
        //
        // A shaper (`Authorize<_, _>`) no longer suppresses this. It masks
        // *fields*, so the caller receives a subset of this shape — which the
        // route records as `masked` and the document says in the response
        // description. Suppressing the schema instead typed every generated
        // client's `#[crud]` response as `any`, on exactly the surface
        // `#[expose]` exists to serve (OAPI-O5).
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
        // The media type of what comes *back*, when it is not JSON. Declared by
        // `#[api(response_content_type = "...")]`, else read off an `-> SSE`
        // return: poem serializes that one type as `text/event-stream` and
        // nothing else, so inferring it states what the framework emits rather
        // than guessing — the same reading that already infers `response` from
        // a `Json<T>` return. A declaration always wins.
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
        // Read off the same type-directed selection the route arms with, so the
        // document cannot claim an unmasked body for a shaper spelled under an
        // alias.
        let masked = quote! { #shaper_selection.is_some() };

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
        let query_param_tys = extractor_payloads(&inputs, "Query");
        let query_params = if query_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#query_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };
        // `Header<T>` payloads, expanded by the document exactly as `Query<T>`
        // is — one parameter per property, `required` off the schema.
        let header_param_tys = extractor_payloads(&inputs, "Header");
        let header_params = if header_param_tys.is_empty() {
            quote! { &[] }
        } else {
            quote! { &[#(::nest_rs_http::schema_of::<#header_param_tys> as ::nest_rs_http::SchemaFn),*] }
        };

        let route_versions = quote! { &[#(#method_versions),*] };
        // A `#[version]` naming something the controller never declared would
        // otherwise mount nowhere — the transport loops over the *controller's*
        // versions — so the route would compile, register, appear in the
        // document, and answer nothing. Assert the subset at the route, in a
        // `const`, so a typo is a compile error where it was typed.
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
                const _: () = ::core::assert!(
                    ::nest_rs_http::versions_declare(<#self_ty>::VERSIONS, #route_versions),
                    #message,
                );
            });
        }

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

    // Per-route layers fold into the access-graph dependencies so an unimported
    // module fails boot with an `AccessGraphError`, not a silent resolution —
    // and carry a label each, so the error names the layer instead of reporting
    // `<unnamed dependency>`. One walk, so the selector below is written once
    // and a seventh layer family cannot misalign keys against labels.
    let route_layers = layer_deps(
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
    let injected_methods = injected_methods_with_layers(&self_ty, &route_layers);

    // Every guard declared beside a verb runs `Guard::check_http`, whose default
    // is `Ok(())` — so one bound per guard, failing at the `#[use_guards]` line
    // rather than passing every request in silence. `#[force_guards]` re-runs an
    // already-composed guard at this route, so it owes the same attestation.
    // Controller-scope guards are bound by `#[controller]`, on the struct where
    // they are written.
    let capability_bounds = guard_capability_bounds(
        routes_by_path
            .iter()
            .flat_map(|(_, handlers)| handlers.iter())
            .flat_map(|handler| handler.guards.iter().chain(&handler.force_guards)),
        quote!(::nest_rs_guards::HttpGuard),
    );

    // Emitted only when this controller actually streams — an unused binding
    // would warn in the developer's build, and the resolve costs a container
    // lookup no non-streaming controller has any use for.
    let has_sse = routes_by_path
        .iter()
        .flat_map(|(_, handlers)| handlers.iter())
        .any(|handler| handler.is_sse);
    let sse_resolve = if has_sse {
        quote! { let __sse = ::nest_rs_http::SseSettings::resolve(container); }
    } else {
        quote! {}
    };

    // One entry per path, built *inside* the per-version loop below: a
    // controller declaring `version = ["1", "2"]` mounts the same handlers at
    // two prefixes, and a `#[version]`-narrowed verb drops out of the versions
    // it does not serve. Each verb is therefore added conditionally, and the
    // path is claimed only if at least one verb survived — an empty method
    // table at a path answers `405`, which is a worse lie than `404`.
    //
    // The table is a `MethodTable` rather than poem's `RouteMethod` because the
    // verb set is the answer a `405` owes (RFC 9110 §15.5.6, a MUST): it is
    // known here, at the declaration, and used to be dropped at the mount. The
    // registering call is what records it, so the served set and the advertised
    // `Allow` cannot drift — including under `#[version]`, where which verbs
    // survive is decided at mount time.
    let route_entries: Vec<TokenStream2> = routes_by_path
        .iter()
        .map(|(path, handlers)| {
            let arms: Vec<TokenStream2> = handlers
                .iter()
                .map(|handler| {
                    let label = format!("{} {}", handler.verb, path.value());
                    let ep = guarded_handler(handler, &label, &self_ty);
                    let verb = &handler.verb;
                    let versions = &handler.versions;
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
                    quote! {
                        if #serves {
                            __method = __method.#verb(#ep);
                        }
                    }
                })
                .collect();
            quote! {
                {
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

    quote! {
        #item

        #capability_bounds

        #(#wrappers)*

        impl ::nest_rs_http::Controller for #self_ty {
            // Routes mount FLAT on the transport's route tree
            // (`.at("<prefix>/<path>")`), not as a nested sub-route: poem's
            // `nest` re-slices the URI and re-routes on every request, and
            // the sub-route's `.data(ctrl)` inserted an extension per
            // request — both pure per-request overhead for a shape known at
            // mount time. `join_path` is the same helper the transport's
            // route log and the OpenAPI document use, so served, logged and
            // documented paths cannot drift.
            fn mount(
                container: &::nest_rs_core::Container,
                route: ::nest_rs_http::poem::Route,
            ) -> ::nest_rs_http::poem::Route {
                let __ctrl = ::std::sync::Arc::new(<#self_ty>::from_container(container));
                // Read once per controller mount, and only when one of its
                // routes streams — see `sse_resolve`.
                #sse_resolve
                let mut __route = route;
                // `[None]` for an unversioned controller: it still mounts, at
                // one address. Iterating an empty list would unmount it.
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
            // `dependencies` stays empty (controller is built at mount); `injected`
            // reports `#[inject]` keys + every container-resolved layer for the
            // access-graph check.
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
    let listed = attr.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
    let elems: Punctuated<Expr, Token![,]> = listed
        .into_iter()
        .map(|lit| -> Expr { parse_quote!(#lit) })
        .collect();
    let array: Expr = parse_quote!([#elems]);
    nest_rs_codegen::versioning::parse_version_list(&array, "#[version]")
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
            nest_rs_codegen::at_most_one_authorize("route"),
        ));
    }
    // Parsed as `Meta`, not as `Path`, so a key this edge cannot express is
    // *reachable*. `Punctuated::<Path, _>` died on the `=` of
    // `bind = Service` with syn's `expected \`,\``, which names neither the key
    // nor the reason — the silence `unmasked` was already lifted out of
    // `PostureRules` to end, with its sibling left behind.
    let metas: Vec<Meta> = attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|_| malformed_authorize(&attr))?
        .into_iter()
        .collect();
    // GraphQL's two, refused by name and with the fact that makes them
    // GraphQL's: they synthesise an id argument and an `Authorized<A, E>` proof,
    // which no other transport can express.
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

/// The shape refusal, worded once: three arms reach it.
fn malformed_authorize(attr: &Attribute) -> syn::Error {
    syn::Error::new_spanned(
        attr,
        "expected `#[authorize(Action, Entity)]` — e.g. \
         `#[authorize(Read, users::Entity)]`. Bind the subject with a \
         `Bind<Service, Action>` parameter when the route loads one",
    )
}

/// The extractor `#[authorize(Action, Entity)]` desugars to — the same
/// parameter `#[crud]` emits on its generated ops, so both paths arm the class
/// gate and the response mask through one mechanism.
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

/// The handler's declared parameter types, in order.
fn param_types(inputs: &[FnArg]) -> Vec<Type> {
    inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pt) => Some((*pt.ty).clone()),
            FnArg::Receiver(_) => None,
        })
        .collect()
}

/// The route's response shaper, selected **by type**: each parameter type is
/// handed to `nest_rs_http::ShaperProbe`, whose two arms the compiler picks
/// between after name resolution. The first parameter that is a
/// `RouteResponseShaper` arms the route; every other type answers `None`.
///
/// This is what makes the arm alias-proof — `use Authorize as Az` changes the
/// spelling, not the type, and the spelling is no longer part of the question.
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

/// A parameter *spelled* `Authorize<..>` / `Bind<..>`, for the HTTP-D1
/// diagnostic only. Arming no longer depends on it (see [`shaper_selection`]):
/// this exists so a type wearing the name but not implementing the trait is a
/// spanned compile error instead of a route that silently arms nothing.
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
        versions: _,
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
    // Constructed at mount time, inside `Controller::mount` where `__ctrl`
    // is in scope — the wrapper endpoint captures the controller `Arc`
    // directly instead of reading it back through a `Data` extension.
    let wrapper_expr = if *is_sse {
        quote! { #wrapper { __ctrl: ::std::sync::Arc::clone(&__ctrl), __sse: __sse } }
    } else {
        quote! { #wrapper { __ctrl: ::std::sync::Arc::clone(&__ctrl) } }
    };
    // Arming is the compiler's answer over the parameter *types*, so a renamed
    // import arms exactly like the canonical spelling. What survives from the
    // old name scan is the run-time probe, now a backstop rather than the net:
    // it catches a masking extractor reached indirectly (nested inside another
    // extractor, or a hand-rolled `FromRequest`), which no type-directed scan
    // of the signature can see. A handler with no extractor parameters at all
    // cannot run one, so its probe is provably dead and is not emitted.
    let shaper_selection = shaper_selection(param_types);
    // HTTP-D1: a parameter *named* `Authorize`/`Bind` that does not implement
    // the shaper trait would select nothing and arm nothing. Assert it eagerly
    // so that is a spanned compile error naming the trait, not a silently
    // unshaped route.
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
    // The three response-side pools compose in ONE call: every additional
    // generic wrapper level would add a `Request`-sized slot to the future
    // poem's route table boxes per request, bare routes included. A route
    // with no response-side layer passes through generic; a layered route
    // goes behind a single box.
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
            ::nest_rs_http::poem::EndpointExt::data(#expr, ::nest_rs_http::Public)
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
    /// `#[api(response = T)]` — the payload the document advertises when the
    /// return type cannot state it: a handler that builds its own `Response`
    /// (the `#[crud]` paginated list, which carries `x-next-cursor`) returns
    /// no `Json<T>` for the macro to read.
    response: Option<Type>,
    /// `#[api(multipart = T)]` — the type describing the parts of a
    /// `multipart/form-data` body. No extractor states them: a handler reads
    /// the parts one at a time (or through its own `FromRequest`), so the form's
    /// shape is declared rather than inferred.
    multipart: Option<Type>,
    /// `#[api(response_content_type = "audio/mpeg")]` — the media type of a
    /// success body that is not JSON, for a handler that builds its own
    /// `Response` (a streamed download, a rendered file).
    response_content_type: Option<LitStr>,
}

/// What `#[api]` accepts, in the error every rejection quotes. One `const` so
/// the list cannot be worded two ways.
const API_KEYS: &str = "#[api] accepts `summary = \"...\"`, `description = \"...\"`, \
                        `tags(\"a\", \"b\")`, `response = Type`, `multipart = Type`, \
                        and `response_content_type = \"type/subtype\"`";

/// Parse `#[api(...)]` straight into [`ApiMeta`].
///
/// **A repeated key is refused**, and on this decorator that matters more than
/// on most: `CLAUDE.md` mandates `#[api(summary = …, description = …)]` *instead
/// of* a doc comment — "Prose the framework compiles into behaviour is declared
/// as an argument, never as a doc comment" — so a dropped `description` is
/// published prose silently replaced by source order.
///
/// Hand-rolled rather than routed through `syn::Meta`: a `Meta::NameValue` holds
/// an **expression**, and `response = Vec<Post>` is a *type* — read as an
/// expression it is a chain of comparisons, so the whole attribute failed with
/// "comparison operators cannot be chained" pointing at the decorator, on a type
/// the developer never wrote.
fn parse_api_attr(attr: &Attribute) -> syn::Result<ApiMeta> {
    attr.parse_args_with(|input: syn::parse::ParseStream<'_>| {
        let mut out = ApiMeta::default();
        while !input.is_empty() {
            let key: syn::Ident = input
                .parse()
                .map_err(|_| syn::Error::new(input.span(), API_KEYS))?;
            match key.to_string().as_str() {
                "summary" => {
                    nest_rs_codegen::reject_duplicate_argument(
                        out.summary.is_some(),
                        &key,
                        "api",
                        "summary",
                    )?;
                    input.parse::<Token![=]>()?;
                    out.summary = Some(require_str_lit(
                        &input.parse::<Expr>()?,
                        "api",
                        "summary",
                        "List users",
                    )?);
                }
                "description" => {
                    nest_rs_codegen::reject_duplicate_argument(
                        out.description.is_some(),
                        &key,
                        "api",
                        "description",
                    )?;
                    input.parse::<Token![=]>()?;
                    out.description = Some(require_str_lit(
                        &input.parse::<Expr>()?,
                        "api",
                        "description",
                        "…",
                    )?);
                }
                "response" => {
                    nest_rs_codegen::reject_duplicate_argument(
                        out.response.is_some(),
                        &key,
                        "api",
                        "response",
                    )?;
                    input.parse::<Token![=]>()?;
                    out.response = Some(input.parse()?);
                }
                "multipart" => {
                    nest_rs_codegen::reject_duplicate_argument(
                        out.multipart.is_some(),
                        &key,
                        "api",
                        "multipart",
                    )?;
                    input.parse::<Token![=]>()?;
                    out.multipart = Some(input.parse()?);
                }
                "response_content_type" => {
                    nest_rs_codegen::reject_duplicate_argument(
                        out.response_content_type.is_some(),
                        &key,
                        "api",
                        "response_content_type",
                    )?;
                    input.parse::<Token![=]>()?;
                    let lit = require_str_lit(
                        &input.parse::<Expr>()?,
                        "api",
                        "response_content_type",
                        "text/csv",
                    )?;
                    check_media_type(&lit)?;
                    out.response_content_type = Some(lit);
                }
                "tags" => {
                    nest_rs_codegen::reject_duplicate_argument(
                        !out.tags.is_empty(),
                        &key,
                        "api",
                        "tags",
                    )?;
                    let content;
                    syn::parenthesized!(content in input);
                    out.tags = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?
                        .into_iter()
                        .collect();
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!("`{other}` is not an #[api] argument — {API_KEYS}"),
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(out)
    })
}

/// The payload type behind an extractor named `name`: `Name<T>`,
/// `Valid<Name<T>>` and `Piped<_, Name<T>>` all yield `T`; anything else yields
/// `None`.
///
/// One reader for every extractor the document describes. It was five —
/// `Json`, `Form`, `Path`, `Query`, `Header` — byte-identical apart from the
/// name, and the cost was not the line count: the two carriers are the
/// framework's pipe grammar, so a change to them (or a sixth extractor) had to
/// land in five places or silently miss one. That is exactly how `Form` came to
/// be absent from the request-body match while every other extractor was read.
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

/// The first `Name<T>` payload in the handler signature — for the extractors a
/// route may only bind once (a request body).
fn first_extractor_payload(inputs: &[FnArg], name: &str) -> Option<Type> {
    extractor_payloads(inputs, name).into_iter().next()
}

/// The path-parameter types a handler binds, in path order. A single
/// `Path<T>` yields `[T]`; a tuple `Path<(A, B)>` yields `[A, B]` (poem binds
/// tuple elements to the `:name` segments left-to-right). A handler with no
/// `Path<…>` extractor (it binds its id via `Bind<_, _>` instead) yields an
/// empty vec — the doc then guesses `format: uuid` for id-like segments.
fn path_param_types(inputs: &[FnArg]) -> Vec<Type> {
    match first_extractor_payload(inputs, "Path") {
        Some(Type::Tuple(tuple)) => tuple.elems.into_iter().collect(),
        Some(other) => vec![other],
        None => Vec::new(),
    }
}

/// Whether a type's last path segment is `name` — the same lightweight,
/// resolution-free match `guard_path_is_throttler` and `result_inner` make. It
/// is what lets a handler binding poem's `Multipart` (or returning its `SSE`)
/// be recognized without this crate depending on poem's types.
fn last_segment_is(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == name))
}

/// Whether the handler pulls the parts itself through poem's `Multipart`.
/// Such a route accepts `multipart/form-data` and nothing types its parts —
/// which is a media type worth documenting even with no schema to go with it.
fn takes_multipart(inputs: &[FnArg]) -> bool {
    inputs.iter().any(|arg| match arg {
        FnArg::Typed(pt) => last_segment_is(&pt.ty, "Multipart"),
        FnArg::Receiver(_) => false,
    })
}

/// Whether the handler returns poem's `SSE`, possibly behind a `Result`. poem
/// serializes that one type as `text/event-stream` and nothing else, so this
/// reports what the framework emits — the same reading that infers a route's
/// response schema from a `Json<T>` return.
fn returns_sse(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };
    last_segment_is(result_inner(ty).unwrap_or(ty), "SSE")
}

/// A media type keys an OpenAPI `content` map, so a malformed one yields a
/// document no client can match a response against. Checked where it is
/// written — `#[api]` has the literal in hand — rather than left to a reader of
/// the generated document.
fn check_media_type(lit: &LitStr) -> syn::Result<()> {
    let value = lit.value();
    // `text/event-stream; charset=utf-8` is a media type with a parameter; the
    // `type/subtype` shape is the part that must be well formed.
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
                "`response_content_type` takes a media type spelled `type/subtype` \
                 — e.g. \"application/octet-stream\", \"text/event-stream\" or \
                 \"audio/mpeg\". `{value}` is not one",
            ),
        ));
    }
    Ok(())
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

    fn api_attr(tokens: TokenStream2) -> syn::Result<ApiMeta> {
        let attr: Attribute = parse_quote!(#[api(#tokens)]);
        parse_api_attr(&attr)
    }

    // `response = Vec<Post>` is a **type**. Read as `syn::Meta` its value would
    // be an expression, and a generic argument list reads there as a chain of
    // comparisons — so the whole `#[crud]` expansion failed with "comparison
    // operators cannot be chained" pointing at the decorator, on a type the
    // developer never wrote.
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

    // A media type keys the document's `content` map. Rejecting a malformed one
    // where it is written beats emitting a document whose response no client
    // can match.
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
        // The two carriers are the framework's pipe grammar, and they are read
        // once for every extractor — so a validated or piped DTO documents
        // itself like a bare one, whichever extractor it sits in.
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
            // And an extractor of another name is not this one.
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

    #[test]
    fn an_unknown_api_argument_names_itself_and_the_accepted_set() {
        let msg = match api_attr(quote! { returns = Post }) {
            Ok(_) => panic!("an unknown key must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(msg.contains("returns"), "{msg}");
        assert!(msg.contains("response = Type"), "{msg}");
    }
}

/// The suffix every controller carries by convention, and which therefore says
/// nothing about *which* controller this is.
const CONTROLLER_SUFFIX: &str = "Controller";

/// `PostsController` → `posts`. A name that is *only* the suffix keeps it,
/// because `_list` names nothing.
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
        // Not every controller type carries the suffix.
        assert_eq!(controller_token("Posts"), "posts");
        // And one that is *only* the suffix keeps it.
        assert_eq!(controller_token("Controller"), "controller");
    }
}
