//! `#[gateway]` — the WebSocket gateway struct decorator (construction + `PATH`
//! const + connection-level guard wrapping). `#[messages]` (in `messages`) emits
//! the `Discoverable`/mount + the dispatcher, since it owns the message table.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, ItemStruct, LitStr, Meta, Path, Token};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_keys_expr, InjectableBody,
};

use crate::attr::{expr_str, take_use_attr};

pub(crate) fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    let path_lit = match parse_gateway_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    // Connection-level guards *on the struct* (the `@UseGuards` analog) — the
    // same `Guard` providers HTTP controllers use, run on the WebSocket upgrade
    // request. An inert attribute consumed here: parse its paths, then strip it
    // so it never reaches the compiler as an unknown attribute.
    let guards = match take_use_attr(&mut item.attrs, "use_guards") {
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
    // The gateway's `#[inject]` keys for the access-graph check. `#[gateway]`
    // sees the fields but `#[messages]` emits the `Discoverable`, so expose them
    // as an inherent fn `#[messages]` reads back into `Discoverable::injected`.
    let injected_keys = injected_keys_expr(&dep_keys);

    // Connection-level guard layers, applied (boxed to a stable type) around the
    // gateway endpoint by `#[messages]`'s mount closure. First listed ends up
    // outermost. With nothing declared it just boxes the endpoint, so
    // `#[messages]` can call it unconditionally.
    let guard_layers = guard_layers(&guards);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            pub const PATH: &'static str = #path_lit;

            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
            }

            #[doc(hidden)]
            pub fn __nestrs_gateway_layers<__E>(
                __container: &::nestrs_core::Container,
                __ep: __E,
            ) -> ::nestrs_ws::poem::endpoint::BoxEndpoint<'static, ::nestrs_ws::poem::Response>
            where
                __E: ::nestrs_ws::poem::Endpoint + 'static,
            {
                let __ep = ::nestrs_ws::poem::EndpointExt::boxed(
                    ::nestrs_ws::poem::EndpointExt::map_to_response(__ep),
                );
                #(#guard_layers)*
                __ep
            }
        }
    }
    .into()
}

/// Parse `#[gateway(path = "/ws")]` — `path` required. Order-independent; an
/// unknown key is rejected with a clear message.
fn parse_gateway_args(args: TokenStream2) -> syn::Result<LitStr> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut path = None;
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => path = Some(expr_str(&nv.value)?),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[gateway] accepts `path = \"...\"`",
                ))
            }
        }
    }
    path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[gateway] requires `path = \"...\"`",
        )
    })
}

/// Build the `let __ep = …;` statements that wrap the gateway endpoint in a list
/// of container-resolved guards via `EndpointExt::guard`, each boxed to the
/// stable `BoxEndpoint` shape. Reversed so the first-listed guard ends up
/// outermost, matching the HTTP convention.
fn guard_layers(paths: &[Path]) -> Vec<TokenStream2> {
    let method = format_ident!("guard");
    paths
        .iter()
        .rev()
        .map(|p| {
            quote! {
                let __ep = ::nestrs_ws::poem::EndpointExt::boxed(
                    ::nestrs_ws::poem::EndpointExt::map_to_response(
                    ::nestrs_ws::EndpointExt::#method(
                        __ep,
                        ::nestrs_core::Container::get::<#p>(__container).expect(concat!(
                            "#[use_guards] guard `",
                            stringify!(#p),
                            "` is not registered — add it to a module's providers"
                        )),
                    ),
                ));
            }
        })
        .collect()
}
