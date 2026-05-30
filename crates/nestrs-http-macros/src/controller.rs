//! `#[controller]` — the controller struct decorator (construction + `PATH`/
//! `VERSION` consts + controller-level interceptor / guard / filter wrapping).
//! `#[routes]` (in `routes`) emits the `Discoverable`/mount, since it owns the
//! route table.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, ItemStruct, LitStr, Meta, Path, Token};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_keys_with_layers, InjectableBody,
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

    // Controller-level interceptor / guard / filter attributes *on the struct*
    // (the class-level `@UseInterceptors` / `@UseGuards` / `@UseFilters` analogs,
    // the same decorators the verb attributes use per route). Each is an inert
    // attribute consumed here — parse its paths, then strip it from the struct so
    // it never reaches the compiler as an unknown attribute (each must sit *below*
    // `#[controller]` for the same reason).
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

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The controller's access-graph dependencies: its `#[inject]` keys *plus* the
    // controller-level guards / filters / interceptors it references. Each layer
    // is resolved from the container at mount (`Container::get::<P>`), exactly like
    // an `#[inject]` dependency, so it belongs under the same boot-time contract —
    // a layer registered in a non-imported module is otherwise resolved silently
    // (a flat-container encapsulation breach), never flagged. `#[controller]` sees
    // the fields and the layer attributes but `#[routes]` emits the `Discoverable`,
    // so expose them as an inherent fn `#[routes]` reads back (and extends with its
    // own per-route layers) into `Discoverable::injected`.
    let injected_keys = injected_keys_with_layers(
        &dep_keys,
        [&interceptors, &guards, &filters].into_iter().flatten(),
    );

    // Controller-level layers: because `mount` is emitted by `#[routes]` (a
    // separate impl block), the lists can't be passed directly — `#[controller]`
    // instead emits an inherent fn that `#[routes]`'s `mount` calls to wrap the
    // controller's whole route subtree. Each layer is boxed to a single
    // `BoxEndpoint` type (the same shape the transport uses for global
    // middleware), so the result type is stable regardless of count. The wrap sits
    // *outside* every per-route layer, so a controller-level layer runs before any
    // route-level one; first listed ends outermost within its layer. With nothing
    // declared it just boxes the endpoint, so `#[routes]` can call it
    // unconditionally. Applied inner → outer as interceptors → guards → filters,
    // mirroring the per-route order (guards run before interceptors; filters wrap
    // both).
    let interceptor_layers = controller_layers(&interceptors, "interceptor", "use_interceptors");
    let guard_layers = controller_layers(&guards, "guard", "use_guards");
    let filter_layers = controller_layers(&filters, "filter", "use_filters");

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
                __container: &::nestrs_core::Container,
                __ep: __E,
            ) -> ::poem::endpoint::BoxEndpoint<'static, ::poem::Response>
            where
                __E: ::poem::Endpoint + 'static,
            {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(__ep));
                #(#interceptor_layers)*
                #(#guard_layers)*
                #(#filter_layers)*
                __ep
            }
        }
    }
    .into()
}

/// Parse `#[controller(path = "...", version = "1")]` — `path` required,
/// `version` optional (URI API versioning, the `@Controller({ version })`
/// analog). Order-independent; an unknown key is rejected with a clear message.
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
                ))
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

/// Build the `let __ep = …;` statements that wrap the controller subtree in a
/// list of container-resolved layers via the `EndpointExt::<method>` extension
/// (`interceptor` / `guard` / `filter`). Each layer is boxed to the stable
/// `BoxEndpoint` shape. Reversed so the first-listed entry ends up outermost
/// within its layer, matching the per-route convention. `attr` names the source
/// attribute for the not-registered diagnostic.
fn controller_layers(paths: &[Path], method: &str, attr: &str) -> Vec<TokenStream2> {
    let method = format_ident!("{method}");
    let prefix = format!("#[{attr}] controller layer `");
    paths
        .iter()
        .rev()
        .map(|p| {
            quote! {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(
                    ::nestrs_http::EndpointExt::#method(
                        __ep,
                        ::nestrs_core::Container::get::<#p>(__container).expect(concat!(
                            #prefix,
                            stringify!(#p),
                            "` is not registered — add it to a module's providers"
                        )),
                    ),
                ));
            }
        })
        .collect()
}
