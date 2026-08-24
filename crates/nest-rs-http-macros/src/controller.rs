//! `#[controller]` — struct decorator (construction + `PATH`/`VERSION` consts +
//! controller-level interceptor/guard/filter wrapping). `#[routes]` owns the
//! route table and emits the `Discoverable`/mount.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{LitStr, Meta, Token};

use nest_rs_codegen::{
    DecoratorPair, InjectableBody, build_injectable_body, from_container_method,
    guard_capability_bounds, injected_keys_with_layers, injected_names_with_layers, layer_deps,
    require_str_lit, scoped_specs, take_path_list,
};

/// The HTTP edge's pair. Read by `#[controller]` here and by `#[routes]` /
/// `#[crud]` next door, so reaching for either half on the wrong shape names the
/// other one instead of reporting syn's `expected struct`.
pub(crate) const HTTP_PAIR: DecoratorPair = DecoratorPair {
    host: "#[controller]",
    subject: "controller struct",
    operations: "#[routes]",
    collects: "#[get] / #[post] / #[put] / #[patch] / #[delete]",
};

pub(crate) fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let (path_lit, versions) = match parse_controller_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let versions_slice = quote! { &[#(#versions),*] };
    let mut item = match HTTP_PAIR.parse_host(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };

    // Inert class-level attributes consumed here; each must sit below `#[controller]`.
    let interceptors = match take_path_list(&mut item.attrs, "use_interceptors") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let guards = match take_path_list(&mut item.attrs, "use_guards") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let filters = match take_path_list(&mut item.attrs, "use_filters") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let pipes = match take_path_list(&mut item.attrs, "use_pipes") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    let exception_filters = match take_path_list(&mut item.attrs, "use_exception_filters") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        ..
    } = match build_injectable_body(&mut item) {
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
    // Keys and their diagnostic labels from one walk: without the labels a layer
    // no module provides is reported as `<unnamed dependency>` — including in
    // the suggested fix — which is precisely the case a `dyn`-injecting guard
    // hits. `layer_deps` keeps the two index-aligned, so this selector is
    // written once.
    let layers = layer_deps(
        [&interceptors, &guards, &filters, &pipes, &exception_filters]
            .into_iter()
            .flatten(),
    );
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);

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
    // Controller-scope guards fold into every route's chain, so they owe the
    // same capability the per-route ones do.
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::HttpGuard));
    // Does a controller-level `#[use_guards]` include `ThrottlerGuard`? `#[routes]`
    // reads this to advertise `429` for every route the controller throttles
    // (OAPI-O4) — a compile-time bool, so the check is free at runtime.
    let controller_has_throttler = guards.iter().any(crate::routes::guard_path_is_throttler);
    let pipe_specs = scoped_specs(&pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let exception_filter_specs = scoped_specs(
        &exception_filters,
        quote!(dyn ::nest_rs_exception_filters::ExceptionFilterErased),
    );

    let residency = HTTP_PAIR.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        impl #impl_generics #name #ty_generics #where_clause {
            /// The controller's route prefix, from `#[controller(path = "…")]`.
            pub const PATH: &'static str = #path_lit;
            /// The versions this controller serves, from
            /// `#[controller(version = …)]`. Empty if unversioned.
            pub const VERSIONS: &'static [&'static str] = #versions_slice;

            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
            }

            #[doc(hidden)]
            pub fn __nestrs_injected_names() -> ::std::vec::Vec<&'static str> {
                #injected_names
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

fn parse_controller_args(args: TokenStream2) -> syn::Result<(LitStr, Vec<LitStr>)> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut path = None;
    let mut versions = Vec::new();
    for meta in metas {
        match meta {
            // Each key answered once. These were plain assignments, so
            // `version = "1", version = "2"` silently kept the last and mounted
            // the controller at an address the developer never wrote — while
            // `version = ["1", "1"]` was already refused, which is the same
            // question asked in the other spelling.
            // The shared sentence, with the remedy appended where there is one
            // — never a second base wording. Both keys asked "declared twice?"
            // in this decorator's own words while `namespace` next door asked it
            // in `nest_rs_codegen::reject_duplicate_argument`'s, so one grammar
            // had two answers.
            Meta::NameValue(nv) if nv.path.is_ident("path") => {
                nest_rs_codegen::reject_duplicate_argument(
                    path.is_some(),
                    &nv,
                    "controller",
                    "path",
                )?;
                path = Some(require_str_lit(&nv.value, "controller", "path", "/users")?);
            }
            Meta::NameValue(nv) if nv.path.is_ident("version") => {
                if !versions.is_empty() {
                    return Err(syn::Error::new_spanned(
                        &nv,
                        format!(
                            "{} — to serve several, write one `version = [\"1\", \"2\"]`",
                            nest_rs_codegen::duplicate_argument("controller", "version"),
                        ),
                    ));
                }
                versions =
                    nest_rs_codegen::versioning::parse_version_list(&nv.value, "#[controller]")?;
            }
            other => {
                return Err(nest_rs_codegen::unmatched_meta(
                    "controller",
                    &other,
                    &["path", "version"],
                ));
            }
        }
    }
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            nest_rs_codegen::missing_argument("controller", "path", "\"/users\""),
        )
    })?;
    nest_rs_codegen::reject_path("controller", &path)?;
    Ok((path, versions))
}
