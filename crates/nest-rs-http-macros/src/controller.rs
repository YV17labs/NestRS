//! `#[controller]` — struct decorator: construction, `PATH`/`VERSIONS` and the
//! controller-level layer specs `#[routes]` reads.

use nest_rs_codegen::__private::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::LitStr;

use nest_rs_codegen::__private::layers::{LayerScope, Layers, take_layers};
use nest_rs_codegen::__private::versioning;
use nest_rs_codegen::__private::{
    InjectableBody, MountPath, RoutePath, build_injectable_body, exception_filter_specs,
    from_container_method, guard_capability_bounds, host_meta_fn, injected_keys_with_layers,
    injected_names_with_layers, injected_optional_fns, injects_impls, interceptor_specs,
    layer_deps, missing_argument, refuse_rust_deprecated, reject_path, scoped_specs,
};

pub(crate) fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let (path_lit, path_parameters, versions) = match parse_controller_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let versions_slice = quote! { &[#(#versions),*] };
    let mut item = match pair::HTTP.parse_host(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    if let Err(err) = refuse_rust_deprecated(&item.attrs, "a `#[controller]`") {
        return err.to_compile_error().into();
    }

    // Inert class-level attributes: each must sit below `#[controller]`.
    let Layers {
        guards,
        pipes,
        interceptors,
        exception_filters,
        meta,
        ..
    } = match take_layers(&pair::HTTP, LayerScope::Host, &mut item.attrs) {
        Ok(layers) => layers,
        Err(err) => return err.to_compile_error().into(),
    };
    let host_meta = host_meta_fn(&meta);

    // Read before the body strips `#[inject]`: `#[crud]` reaches its service
    // through the field of its type.
    let injected_fields = match injects_impls(&item, "controller") {
        Ok(impls) => impls,
        Err(err) => return err.to_compile_error().into(),
    };
    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
        opt_names,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // A layer is resolved at mount, so it joins the access graph like a field;
    // otherwise one from a non-imported module resolves through the flat container.
    let layers = layer_deps(
        [&interceptors, &guards, &pipes, &exception_filters]
            .into_iter()
            .flatten(),
    );
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);
    let injected_optional = injected_optional_fns(&opt_keys, &opt_names);

    let interceptor_specs = interceptor_specs(&interceptors);
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    let capability_bounds = guard_capability_bounds(
        guards.iter(),
        quote!(::nest_rs_guards::GuardFor<::nest_rs_http::HttpView>),
    );
    let pipe_specs = scoped_specs(&pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let exception_filter_specs = exception_filter_specs(&exception_filters);

    let residency = pair::HTTP.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        #injected_fields

        impl #impl_generics #name #ty_generics #where_clause {
            /// The controller's route prefix, from `#[controller(path = "…")]`.
            pub const PATH: &'static str = #path_lit;
            /// The versions this controller serves, from
            /// `#[controller(version = …)]`. Empty if unversioned.
            pub const VERSIONS: &'static [&'static str] = #versions_slice;

            /// How many parameters the controller's path names, read by
            /// `#[routes]` and `#[crud]` where a route reads its id.
            #[doc(hidden)]
            pub const fn __nestrs_controller_path_parameters() -> usize {
                #path_parameters
            }

            #from_container

            #host_meta

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
            }

            #[doc(hidden)]
            pub fn __nestrs_injected_names() -> ::std::vec::Vec<&'static str> {
                #injected_names
            }

            #injected_optional

            /// Controller-level `#[use_interceptors(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_interceptor_specs()
                -> ::std::vec::Vec<::nest_rs_core::__private::InterceptorSpec>
            {
                #interceptor_specs
            }

            /// Controller-level `#[use_guards(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::__private::GuardSpec>
            {
                #guard_specs
            }

            /// Controller-level `#[use_pipes(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_pipe_specs()
                -> ::std::vec::Vec<::nest_rs_http::__private::PipeSpec>
            {
                #pipe_specs
            }

            /// Controller-level `#[use_exception_filters(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_exception_filter_specs()
                -> ::std::vec::Vec<::nest_rs_core::__private::ExceptionFilterSpec>
            {
                #exception_filter_specs
            }
        }
    }
    .into()
}

const CONTROLLER: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("controller", &["path", "version"]).with_remedies(&[(
        "version",
        "to serve several, write one `version = [\"1\", \"2\"]`",
    )]);

/// The path, how many parameters it names, and the versions served.
fn parse_controller_args(args: TokenStream2) -> syn::Result<(LitStr, usize, Vec<LitStr>)> {
    let mut path = None;
    let mut versions = Vec::new();
    CONTROLLER.parse2(args, |arg| {
        match arg.key() {
            "path" => path = Some(arg.str_lit("/users")?),
            "version" => {
                versions = versioning::parse_version_list(&arg.expr()?, "#[controller]")?;
            }
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            missing_argument("controller", "path", "\"/users\""),
        )
    })?;
    reject_path("controller", &path, MountPath::Prefix)?;
    let parameters = RoutePath::parse(&path.value())
        .map_err(|why| syn::Error::new_spanned(&path, why))?
        .parameter_count();
    Ok((path, parameters, versions))
}
