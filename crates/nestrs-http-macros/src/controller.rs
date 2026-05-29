//! `#[controller]` — the controller struct decorator (construction + `PATH`/
//! `VERSION` consts + controller-level guard wrapping). `#[routes]` (in `routes`)
//! emits the `Discoverable`/mount, since it owns the route table.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Attribute, ItemStruct, LitStr, Meta, Path, Token};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_keys_expr, InjectableBody,
};

use crate::attr::expr_str;

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

    // Controller-level guards: a `#[use_guards(GuardA, GuardB)]` attribute *on the
    // struct* (the class-level `@UseGuards` analog, the same decorator the verb
    // attributes use per route). It is an inert attribute consumed here — parse its
    // paths, then strip it from the struct so it never reaches the compiler as an
    // unknown attribute (it must sit *below* `#[controller]` for the same reason).
    let guards = match take_use_guards(&mut item.attrs) {
        Ok(guards) => guards,
        Err(err) => return err.to_compile_error().into(),
    };

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The controller's `#[inject]` keys for the access-graph check.
    // `#[controller]` sees the fields but `#[routes]` emits the
    // `Discoverable`, so expose them as an inherent fn `#[routes]` reads back
    // into `Discoverable::injected`.
    let injected_keys = injected_keys_expr(&dep_keys);

    // Controller-level guards: because `mount` is emitted by `#[routes]` (a
    // separate impl block), the guard list can't be passed directly —
    // `#[controller]` instead emits this inherent fn that `#[routes]`'s `mount`
    // calls to wrap the controller's whole route subtree. Each layer is boxed to a
    // single `BoxEndpoint` type (the same shape the transport uses for global
    // guards), so the result type is stable regardless of guard count. The wrap
    // sits *outside* every per-route guard/shaper, so a controller guard (e.g.
    // `AuthGuard`) runs before any route-level one; first listed ends outermost.
    // With no guards it just boxes the endpoint, so `#[routes]` can call it
    // unconditionally.
    let guard_layers: Vec<TokenStream2> = guards
        .iter()
        .rev()
        .map(|g| {
            quote! {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(
                    ::nestrs_http::EndpointExt::guard(
                        __ep,
                        ::nestrs_core::Container::get::<#g>(__container).expect(concat!(
                            "#[use_guards] controller guard `",
                            stringify!(#g),
                            "` is not registered — add it to a module's providers"
                        )),
                    ),
                ));
            }
        })
        .collect();

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
            pub fn __nestrs_controller_guards<__E>(
                __container: &::nestrs_core::Container,
                __ep: __E,
            ) -> ::poem::endpoint::BoxEndpoint<'static, ::poem::Response>
            where
                __E: ::poem::Endpoint + 'static,
            {
                let __ep = ::poem::EndpointExt::boxed(::poem::EndpointExt::map_to_response(__ep));
                #(#guard_layers)*
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

/// Extract and remove a controller-level `#[use_guards(GuardA, GuardB)]` attribute
/// from a struct's attribute list (the class-level `@UseGuards` analog). Returns
/// the guard paths (empty when absent). The attribute is *consumed* — removed from
/// `attrs` so it never reaches the compiler as an unknown attribute, the same way
/// `#[routes]` consumes the method-level form. At most one is accepted.
fn take_use_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("use_guards")) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident("use_guards")) {
        return Err(syn::Error::new_spanned(
            &attr,
            "a controller takes at most one `#[use_guards(...)]`; list every guard in it",
        ));
    }
    Ok(attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}
