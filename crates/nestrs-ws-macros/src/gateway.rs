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
    build_injectable_body, from_container_method, injected_keys_with_layers, InjectableBody,
};

use crate::attr::{expr_str, take_use_attr};

pub(crate) fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    let GatewayArgs { path, namespace } = match parse_gateway_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let path_lit = path;
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
    // The gateway's access-graph dependencies: its `#[inject]` keys *plus* the
    // connection-level guards it references — each container-resolved at mount
    // (`Container::get::<P>`) on the upgrade request, so a guard registered in a
    // non-imported module must fail the boot like an out-of-reach `#[inject]`
    // rather than resolve silently through the flat container. `#[gateway]` sees
    // the fields and the guard attribute but `#[messages]` emits the
    // `Discoverable`, so expose them as an inherent fn `#[messages]` reads back
    // (and extends with its per-message guards) into `Discoverable::injected`.
    let injected_keys = injected_keys_with_layers(&dep_keys, guards.iter());

    // Connection-level guard layers, applied (boxed to a stable type) around the
    // gateway endpoint by `#[messages]`'s mount closure. First listed ends up
    // outermost. With nothing declared it just boxes the endpoint, so
    // `#[messages]` can call it unconditionally.
    let guard_layers = guard_layers(&guards);

    // The gateway's connection-registry namespace marker (`#[gateway(namespace =
    // MyNs)]`), defaulting to the shared `Global` registry `WsModule` provides. A
    // namespaced gateway owns a *separate* `WsServer<MyNs>` it self-provides, so
    // its broadcasts never reach another gateway's clients. The namespace lives
    // entirely here; `#[messages]` resolves and provides it through two inherent
    // helpers, never naming the marker itself.
    let ns_ty = match &namespace {
        Some(path) => quote! { #path },
        None => quote! { ::nestrs_ws::Global },
    };
    let provide_registry = match &namespace {
        // A namespaced registry is self-provided so listing the gateway in
        // `providers` is all the wiring there is.
        Some(_) => quote! {
            ::nestrs_core::ContainerBuilder::provide(
                __builder,
                <::nestrs_ws::WsServer<#ns_ty>>::default(),
            )
        },
        // The `Global` registry comes from `WsModule`; nothing to provide here.
        None => quote! { __builder },
    };

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
            pub fn __nestrs_registry(
                __container: &::nestrs_core::Container,
            ) -> ::std::sync::Arc<::nestrs_ws::WsServer<#ns_ty>> {
                ::nestrs_core::Container::get::<::nestrs_ws::WsServer<#ns_ty>>(__container).expect(
                    "WebSocket gateway requires its connection registry — add `WsModule` to a \
                     module's `imports` for the default namespace, or the gateway self-provides \
                     a `namespace`d one",
                )
            }

            #[doc(hidden)]
            pub fn __nestrs_provide_registry(
                __builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                #provide_registry
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

/// Parsed `#[gateway(...)]` arguments: the required mount `path` and the optional
/// connection-registry `namespace` marker type.
struct GatewayArgs {
    path: LitStr,
    namespace: Option<Path>,
}

/// Parse `#[gateway(path = "/ws", namespace = MyNs)]` — `path` required,
/// `namespace` optional (a marker type, default the shared `Global` registry).
/// Order-independent; an unknown key is rejected with a clear message.
fn parse_gateway_args(args: TokenStream2) -> syn::Result<GatewayArgs> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut path = None;
    let mut namespace = None;
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => path = Some(expr_str(&nv.value)?),
            Meta::NameValue(nv) if nv.path.is_ident("namespace") => {
                namespace = Some(expr_path(&nv.value)?)
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[gateway] accepts `path = \"...\"` and an optional `namespace = MarkerType`",
                ))
            }
        }
    }
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[gateway] requires `path = \"...\"`",
        )
    })?;
    Ok(GatewayArgs { path, namespace })
}

/// A `namespace = Type` value must be a bare type path.
fn expr_path(expr: &syn::Expr) -> syn::Result<Path> {
    match expr {
        syn::Expr::Path(p) => Ok(p.path.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            "`namespace` expects a marker type path, e.g. `namespace = ChatNs`",
        )),
    }
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
