//! HTTP decorator macros, re-exported by `nestrs-http` so apps write
//! `nestrs_http::controller` etc. The generated code uses absolute paths
//! (`::nestrs_http::*`, `::poem::*`, `::nestrs_core::*`), so this crate does
//! not depend on those crates — they resolve at the call site.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, ItemStruct, LitStr, Path, ReturnType};

use nestrs_macro_support::{
    build_injectable_body, dependencies_method, forwarded_arg_idents, from_container_method,
    parse_named_str_arg, InjectableBody,
};

/// One route handler in a controller: its HTTP verb ident, the generated
/// wrapper-fn ident, and the guard paths declared with `#[use_guards]`.
type RouteHandler = (syn::Ident, syn::Ident, Vec<Path>);

/// Handlers grouped by path in first-seen order — several verbs may share a path
/// (`GET` + `POST /users`), which `#[routes]` collapses into one `RouteMethod`.
type RoutesByPath = Vec<(LitStr, Vec<RouteHandler>)>;

// -----------------------------------------------------------------------------
// #[controller(path = "...")]
// -----------------------------------------------------------------------------

/// `#[controller(path = "/health")]` — paired with `#[routes]` on the impl block.
///
/// Generates a `from_container(&Container) -> Self` constructor and a
/// `pub const PATH: &'static str` used by `#[routes]` as the route prefix.
///
/// The `Discoverable` impl is emitted by `#[routes]` rather than here — it
/// needs the route table that `#[routes]` collects, and emitting it in two
/// places would conflict.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let path_lit = match parse_named_str_arg(args.into(), "path", "controller") {
        Ok(path) => path,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody { ctor, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            pub const PATH: &'static str = #path_lit;

            #from_container
        }
    }
    .into()
}

// -----------------------------------------------------------------------------
// #[interceptor]
// -----------------------------------------------------------------------------

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

    let InjectableBody { ctor, dep_keys } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let dependencies = dependencies_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies

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

// -----------------------------------------------------------------------------
// #[routes]
// -----------------------------------------------------------------------------

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

        let handler = (verb_ident.clone(), wrapper_name.clone(), guards);
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

        route_metas.push(quote! {
            ::nestrs_http::HttpRouteMeta {
                verb: #verb_variant,
                path: #route_path,
                handler: #method_name_lit,
            }
        });
    }

    // One `.at(path, RouteMethod)` per path: the first verb opens the
    // `RouteMethod`, the rest chain onto it (`get(h1).post(h2)`).
    let route_entries: Vec<TokenStream2> = routes_by_path
        .iter()
        .map(|(path, handlers)| {
            let mut handlers = handlers.iter();
            let (first_verb, first_wrapper, first_guards) =
                handlers.next().expect("each path has at least one verb");
            let first = guarded_handler(first_wrapper, first_guards);
            let mut method = quote! { ::poem::#first_verb(#first) };
            for (verb, wrapper, guards) in handlers {
                let ep = guarded_handler(wrapper, guards);
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
                route.nest(<#self_ty>::PATH, __sub)
            }
        }

        impl ::nestrs_core::Discoverable for #self_ty {
            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                let __meta = ::nestrs_http::HttpControllerMeta::new(
                    <#self_ty>::PATH,
                    ::std::vec![#(#route_metas),*],
                    |__c, __r| <#self_ty as ::nestrs_http::Controller>::mount(__c, __r),
                );
                builder.attach_meta::<#self_ty, ::nestrs_http::HttpControllerMeta>(__meta)
            }
        }
    }
    .into()
}

/// Wrap a route handler in its `#[use_guards]` guards, each resolved from the
/// container at mount time. The first guard listed ends up outermost, so it runs
/// first; with no guards the handler is emitted unchanged. Generated inside
/// `Controller::mount`, where `container: &Container` is in scope.
fn guarded_handler(wrapper: &syn::Ident, guards: &[Path]) -> TokenStream2 {
    let mut expr = quote! { #wrapper };
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
    expr
}
