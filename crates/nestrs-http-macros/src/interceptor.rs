//! `#[interceptor]` — mark a struct as a **global** HTTP interceptor the framework
//! discovers and wraps around the whole route tree (for infrastructure that must
//! wrap everything: a DB-transaction context, tracing). It attaches an
//! [`HttpInterceptorMeta`](::nestrs_http::HttpInterceptorMeta) but does *not*
//! register the concrete type as a provider, so it is mounted automatically and
//! is not referenced by name.
//!
//! To bind an interceptor to a single controller or handler instead, do *not*
//! use this macro: write a plain `#[injectable] + impl Interceptor` (symmetric to
//! how a guard is `#[injectable] + impl Guard`) and list it in
//! `#[use_interceptors(...)]` on the controller struct or beside a verb attribute.
//! It is then resolved from the container at mount time, exactly like a
//! `#[use_guards]` guard.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

use nestrs_codegen::{
    build_injectable_body, dependencies_method, dependency_names_method, from_container_method,
    injected_method, optional_dependencies_method, InjectableBody,
};

pub(crate) fn interceptor(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let dependencies = dependencies_method(&dep_keys);
    let dependency_names = dependency_names_method(&dep_names);
    let optional_dependencies = optional_dependencies_method(&opt_keys);
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected

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
