//! `#[controller]` — struct decorator: construction, `PATH`/`VERSIONS` and the
//! controller-level layer specs `#[routes]` reads.

use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::LitStr;

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, from_container_method, guard_capability_bounds,
    injected_keys_with_layers, injected_names_with_layers, layer_deps, scoped_specs,
    take_path_list,
};

pub(crate) fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let (path_lit, versions) = match parse_controller_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let versions_slice = quote! { &[#(#versions),*] };
    let mut item = match pair::HTTP.parse_host(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    if let Err(err) = nest_rs_codegen::refuse_rust_deprecated(&item.attrs, "a `#[controller]`") {
        return err.to_compile_error().into();
    }

    // Inert class-level attributes: each must sit below `#[controller]`.
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
    // A layer is resolved at mount, so it joins the access graph like a field;
    // otherwise one from a non-imported module resolves through the flat container.
    let layers = layer_deps(
        [&interceptors, &guards, &filters, &pipes, &exception_filters]
            .into_iter()
            .flatten(),
    );
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);

    let interceptor_specs = scoped_specs(
        &interceptors,
        quote!(dyn ::nest_rs_interceptors::Interceptor),
    );
    let filter_specs = scoped_specs(&filters, quote!(dyn ::nest_rs_filters::Filter));
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::HttpGuard));
    let controller_has_throttler = guards.iter().any(crate::routes::guard_path_is_throttler);
    let pipe_specs = scoped_specs(&pipes, quote!(dyn ::nest_rs_pipes::GlobalPipe));
    let exception_filter_specs = scoped_specs(
        &exception_filters,
        quote!(dyn ::nest_rs_exception_filters::ExceptionFilterErased),
    );

    let residency = pair::HTTP.host_residency(&name, &item.generics);

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

            /// Controller-level `#[use_interceptors(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_interceptor_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedInterceptorSpec>
            {
                #interceptor_specs
            }

            /// Controller-level `#[use_filters(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_filter_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedFilterSpec>
            {
                #filter_specs
            }

            /// Controller-level `#[use_guards(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                #guard_specs
            }

            /// Whether a controller-level `#[use_guards(...)]` includes
            /// `ThrottlerGuard`, so `#[routes]` advertises a `429`.
            #[doc(hidden)]
            pub fn __nestrs_controller_has_throttler() -> bool {
                #controller_has_throttler
            }

            /// Controller-level `#[use_pipes(...)]`, read by `#[routes]`.
            #[doc(hidden)]
            pub fn __nestrs_controller_pipe_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedPipeSpec>
            {
                #pipe_specs
            }

            /// Controller-level `#[use_exception_filters(...)]`, read by `#[routes]`.
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

const CONTROLLER: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("controller", &["path", "version"]).with_remedies(&[(
        "version",
        "to serve several, write one `version = [\"1\", \"2\"]`",
    )]);

fn parse_controller_args(args: TokenStream2) -> syn::Result<(LitStr, Vec<LitStr>)> {
    let mut path = None;
    let mut versions = Vec::new();
    CONTROLLER.parse2(args, |arg| {
        match arg.key() {
            "path" => path = Some(arg.str_lit("/users")?),
            "version" => {
                versions =
                    nest_rs_codegen::versioning::parse_version_list(&arg.expr()?, "#[controller]")?;
            }
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            nest_rs_codegen::missing_argument("controller", "path", "\"/users\""),
        )
    })?;
    nest_rs_codegen::reject_path("controller", &path, nest_rs_codegen::MountPath::Prefix)?;
    Ok((path, versions))
}
