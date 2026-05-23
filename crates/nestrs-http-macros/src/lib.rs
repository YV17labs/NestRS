//! HTTP decorator macros, re-exported by `nestrs-http` so apps write
//! `nestrs_http::controller` etc. The generated code uses absolute paths
//! (`::nestrs_http::*`, `::poem::*`, `::nestrs_core::*`), so this crate does
//! not depend on those crates — they resolve at the call site.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, ItemStruct, LitStr, ReturnType};

use nestrs_macro_support::{
    build_injectable_body, dependencies_method, forwarded_arg_idents, from_container_method,
    parse_path_arg, InjectableBody,
};

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
    let path_lit = match parse_path_arg(args.into(), "controller") {
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
    let mut route_entries: Vec<TokenStream2> = Vec::new();
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

        route_entries.push(quote! {
            .at(#route_path, ::poem::#verb_ident(#wrapper_name))
        });

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
