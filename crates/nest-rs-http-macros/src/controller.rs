//! `#[controller]` — struct decorator (construction + `PATH`/`VERSION` consts +
//! controller-level interceptor/guard/filter wrapping). `#[routes]` owns the
//! route table and emits the `Discoverable`/mount.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{ItemStruct, LitStr, Meta, Path, Token, parse_macro_input};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, from_container_method, injected_keys_with_layers,
};

use crate::attr::{expr_str, take_use_attr};

pub(crate) fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let (path_lit, version) = match parse_controller_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let version_opt = match &version {
        Some(v) => quote! { ::core::option::Option::Some(#v) },
        None => quote! { ::core::option::Option::None },
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    // Inert class-level attributes consumed here; each must sit below `#[controller]`.
    let interceptors = match take_use_attr(&mut item.attrs, "use_interceptors") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let guards = match take_use_attr(&mut item.attrs, "use_guards") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let filters = match take_use_attr(&mut item.attrs, "use_filters") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let pipes = match take_use_attr(&mut item.attrs, "use_pipes") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let exception_filters = match take_use_attr(&mut item.attrs, "use_exception_filters") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // Access-graph dependencies: `#[inject]` keys + controller-level layers.
    // Each layer is `Container::get::<P>` at mount, so it must be checked under
    // the same boot contract as a field — otherwise a layer registered in a
    // non-imported module resolves silently (flat-container leak). `#[routes]`
    // owns `Discoverable`, so the keys are exposed via an inherent fn it reads.
    let injected_keys = injected_keys_with_layers(
        &dep_keys,
        [&interceptors, &guards, &filters, &pipes, &exception_filters]
            .into_iter()
            .flatten(),
    );

    // `mount` is emitted by `#[routes]` (separate impl), so the layer lists are
    // exposed via an inherent fn `#[routes]` calls. Each layer is boxed to a
    // single `BoxEndpoint` so the result type stays stable regardless of count;
    // wrap sits outside every per-route layer (first listed outermost within its
    // layer). Applied inner→outer as interceptors → guards → filters, matching
    // the per-route order. Guards stay as a controller-level wrap **only** so
    // the controller's `#[use_guards]` participates in the per-route Layer
    // System dedup via `__nestrs_controller_guard_specs()`; the wrap below
    // simply boxes the endpoint without adding a guard, so we'd otherwise drop
    // the helper entirely. We keep the box for type stability across handlers.
    let interceptor_layers = controller_interceptor_layers(&interceptors);
    let filter_layers = controller_filter_layers(&filters);
    let guard_specs = controller_guard_specs(&guards);
    let pipe_specs = controller_pipe_specs(&pipes);
    let exception_filter_specs = controller_exception_filter_specs(&exception_filters);

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

            #[doc(hidden)]
            pub fn __nestrs_controller_layers<__E>(
                __container: &::nest_rs_core::Container,
                __ep: __E,
            ) -> ::poem::endpoint::BoxEndpoint<'static, ::poem::Response>
            where
                __E: ::poem::Endpoint + 'static,
            {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(__ep));
                #(#interceptor_layers)*
                #(#filter_layers)*
                __ep
            }

            /// Controller-level `#[use_guards(...)]`, exposed for the
            /// `#[routes]` macro to fold into each route's
            /// `LayersRouteInterceptor`. Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::integration::RouteGuardSpec>
            {
                #guard_specs
            }

            /// Controller-level `#[use_pipes(...)]`, exposed for the
            /// `#[routes]` macro to fold into each route's
            /// `LayersRouteInterceptor`. Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_pipe_specs()
                -> ::std::vec::Vec<::nest_rs_guards::integration::RoutePipeSpec>
            {
                #pipe_specs
            }

            /// Controller-level `#[use_exception_filters(...)]`, exposed for
            /// the `#[routes]` macro to fold into each route's
            /// `LayersRouteInterceptor`. Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_exception_filter_specs()
                -> ::std::vec::Vec<::nest_rs_guards::integration::RouteExceptionFilterSpec>
            {
                #exception_filter_specs
            }
        }
    }
    .into()
}

/// Parse `#[controller(path = "...", version = "1")]` — `path` required,
/// `version` optional. Order-independent; unknown keys rejected.
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
                ));
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

/// Controller-level `#[use_interceptors(...)]` wrap statements. Each layer is
/// boxed to `BoxEndpoint`; reversed so the first-listed entry ends up
/// outermost within its layer.
fn controller_interceptor_layers(paths: &[Path]) -> Vec<TokenStream2> {
    paths
        .iter()
        .rev()
        .map(|p| {
            quote! {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(
                    ::nest_rs_http::InterceptorExt::interceptor(
                        __ep,
                        ::nest_rs_core::Container::get::<#p>(__container).expect(concat!(
                            "#[use_interceptors] controller layer `",
                            stringify!(#p),
                            "` is not registered — add it to a module's providers"
                        )),
                    ),
                ));
            }
        })
        .collect()
}

/// Controller-level `#[use_filters(...)]` wrap statements.
fn controller_filter_layers(paths: &[Path]) -> Vec<TokenStream2> {
    paths
        .iter()
        .rev()
        .map(|p| {
            quote! {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(
                    ::nest_rs_http::FilterExt::filter(
                        __ep,
                        ::nest_rs_core::Container::get::<#p>(__container).expect(concat!(
                            "#[use_filters] controller layer `",
                            stringify!(#p),
                            "` is not registered — add it to a module's providers"
                        )),
                    ),
                ));
            }
        })
        .collect()
}

/// Controller-level `#[use_guards(...)]` → `Vec<RouteGuardSpec>` so `#[routes]`
/// folds each guard into the per-route `LayersRouteInterceptor` and the Layer
/// System dedup sees the controller scope.
fn controller_guard_specs(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec::Vec::new() };
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

/// Controller-level `#[use_pipes(...)]` → `Vec<RoutePipeSpec>`.
fn controller_pipe_specs(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec::Vec::new() };
    }
    let entries = paths.iter().map(|p| {
        quote! {
            ::nest_rs_guards::integration::RouteLayerSpec {
                type_id: ::core::any::TypeId::of::<#p>(),
                name: ::core::any::type_name::<#p>(),
                resolve: |__c| ::nest_rs_core::Container::get::<#p>(__c)
                    .map(|__arc| __arc as ::std::sync::Arc<dyn ::nest_rs_pipes::GlobalPipe>),
            }
        }
    });
    quote! { ::std::vec![#(#entries),*] }
}

/// Controller-level `#[use_exception_filters(...)]` →
/// `Vec<RouteExceptionFilterSpec>`. Each entry erases the filter to
/// `dyn ExceptionFilterErased` via its blanket impl.
fn controller_exception_filter_specs(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec::Vec::new() };
    }
    let entries = paths.iter().map(|p| {
        quote! {
            ::nest_rs_guards::integration::RouteLayerSpec {
                type_id: ::core::any::TypeId::of::<#p>(),
                name: ::core::any::type_name::<#p>(),
                resolve: |__c| ::nest_rs_core::Container::get::<#p>(__c)
                    .map(|__arc| __arc as ::std::sync::Arc<dyn ::nest_rs_exception_filters::ExceptionFilterErased>),
            }
        }
    });
    quote! { ::std::vec![#(#entries),*] }
}
