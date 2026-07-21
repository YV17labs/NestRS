//! `#[controller]` — struct decorator (construction + `PATH`/`VERSION` consts +
//! controller-level interceptor/guard/filter wrapping). `#[routes]` owns the
//! route table and emits the `Discoverable`/mount.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{ItemStruct, LitStr, Meta, Token, parse_macro_input};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, expr_str, from_container_method,
    injected_keys_with_layers, scoped_specs, take_path_list,
};

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
    let interceptors = match take_path_list(&mut item.attrs, "use_interceptors", "entry") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let guards = match take_path_list(&mut item.attrs, "use_guards", "entry") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let filters = match take_path_list(&mut item.attrs, "use_filters", "entry") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let pipes = match take_path_list(&mut item.attrs, "use_pipes", "entry") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let exception_filters = match take_path_list(&mut item.attrs, "use_exception_filters", "entry")
    {
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
    // layer). Per-route nesting (inner→outer) is built by `#[routes]`:
    // handler → ability shaper → interceptors → filters → RouteShaper → meta.
    // Guards stay as a controller-level wrap **only** so
    // the controller's `#[use_guards]` participates in the per-route Layer
    // System dedup via `__nestrs_controller_guard_specs()`; the wrap below
    // simply boxes the endpoint without adding a guard, so we'd otherwise drop
    // the helper entirely. We keep the box for type stability across handlers.
    let interceptor_specs = scoped_specs(
        &interceptors,
        quote!(dyn ::nest_rs_interceptors::Interceptor),
    );
    let filter_specs = scoped_specs(&filters, quote!(dyn ::nest_rs_filters::Filter));
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    // Does a controller-level `#[use_guards]` include `ThrottlerGuard`? `#[routes]`
    // reads this to advertise `429` for every route the controller throttles
    // (OAPI-O4) — a compile-time bool, so the check is free at runtime.
    let controller_has_throttler = guards.iter().any(crate::routes::guard_path_is_throttler);
    let pipe_specs = scoped_specs(&pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let exception_filter_specs = scoped_specs(
        &exception_filters,
        quote!(dyn ::nest_rs_exception_filters::ExceptionFilterErased),
    );

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            /// The controller's route prefix, from `#[controller(path = "…")]`.
            pub const PATH: &'static str = #path_lit;
            /// The URI version segment, from `#[controller(version = "…")]`; `None` if unversioned.
            pub const VERSION: ::core::option::Option<&'static str> = #version_opt;

            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
            }

            /// Controller-level `#[use_interceptors(...)]`, exposed for the
            /// `#[routes]` macro to compose into each route's interceptor pool
            /// (`wrap_route_interceptors`). Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_interceptor_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedInterceptorSpec>
            {
                #interceptor_specs
            }

            /// Controller-level `#[use_filters(...)]`, exposed for the
            /// `#[routes]` macro to compose into each route's filter pool
            /// (`wrap_route_filters`). Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_filter_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedFilterSpec>
            {
                #filter_specs
            }

            /// Controller-level `#[use_guards(...)]`, exposed for the
            /// `#[routes]` macro to fold into each route's
            /// `RouteShaper`. Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                #guard_specs
            }

            /// Whether a controller-level `#[use_guards(...)]` includes
            /// `ThrottlerGuard`, so `#[routes]` can advertise a `429` for every
            /// route this controller throttles (OAPI-O4). A compile-time
            /// constant folded into each route's `throttled` flag.
            #[doc(hidden)]
            pub fn __nestrs_controller_has_throttler() -> bool {
                #controller_has_throttler
            }

            /// Controller-level `#[use_pipes(...)]`, exposed for the
            /// `#[routes]` macro to fold into each route's
            /// `RouteShaper`. Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_pipe_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedPipeSpec>
            {
                #pipe_specs
            }

            /// Controller-level `#[use_exception_filters(...)]`, exposed for
            /// the `#[routes]` macro to fold into each route's
            /// `RouteShaper`. Empty when none are declared.
            #[doc(hidden)]
            pub fn __nestrs_controller_exception_filter_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedExceptionFilterSpec>
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
