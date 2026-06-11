//! `#[gateway]` — struct decorator (construction + `PATH` + connection-level
//! guard wrapping). `#[messages]` emits the `Discoverable`/mount + dispatcher.

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

pub(crate) fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    let GatewayArgs { path, namespace } = match parse_gateway_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let path_lit = path;
    let mut item = parse_macro_input!(input as ItemStruct);

    // `@UseGuards` analog on the struct — run on the WS upgrade.
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
    // Access-graph deps: `#[inject]` keys + connection-level guards. Exposed
    // through an inherent fn `#[messages]` reads back (and extends with its
    // per-message guards) when emitting `Discoverable::injected`.
    let injected_keys = injected_keys_with_layers(&dep_keys, guards.iter());

    // Connection-level guard layers; first listed ends up outermost. With
    // nothing declared this just boxes the endpoint.
    let guard_layers = guard_layers(&guards);

    let ns_ty = match &namespace {
        Some(path) => quote! { #path },
        None => quote! { ::nest_rs_ws::Global },
    };
    let provide_registry = match &namespace {
        // A namespaced gateway self-provides its `WsServer<Ns>`; `Global`
        // comes from `WsModule`.
        Some(_) => quote! {
            ::nest_rs_core::ContainerBuilder::provide(
                __builder,
                <::nest_rs_ws::WsServer<#ns_ty>>::default(),
            )
        },
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
                __container: &::nest_rs_core::Container,
            ) -> ::std::sync::Arc<::nest_rs_ws::WsServer<#ns_ty>> {
                ::nest_rs_core::Container::get::<::nest_rs_ws::WsServer<#ns_ty>>(__container).expect(
                    "WebSocket gateway requires its connection registry — add `WsModule` to a \
                     module's `imports` for the default namespace, or the gateway self-provides \
                     a `namespace`d one",
                )
            }

            #[doc(hidden)]
            pub fn __nestrs_provide_registry(
                __builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                #provide_registry
            }

            #[doc(hidden)]
            pub fn __nestrs_gateway_layers<__E>(
                __container: &::nest_rs_core::Container,
                __ep: __E,
            ) -> ::nest_rs_ws::poem::endpoint::BoxEndpoint<'static, ::nest_rs_ws::poem::Response>
            where
                __E: ::nest_rs_ws::poem::Endpoint + 'static,
            {
                let __ep = ::nest_rs_ws::poem::EndpointExt::boxed(
                    ::nest_rs_ws::poem::EndpointExt::map_to_response(__ep),
                );
                #(#guard_layers)*
                __ep
            }
        }
    }
    .into()
}

struct GatewayArgs {
    path: LitStr,
    namespace: Option<Path>,
}

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
                ));
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

fn expr_path(expr: &syn::Expr) -> syn::Result<Path> {
    match expr {
        syn::Expr::Path(p) => Ok(p.path.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            "`namespace` expects a marker type path, e.g. `namespace = ChatNs`",
        )),
    }
}

/// Reversed so the first-listed guard ends up outermost (HTTP convention).
///
/// Resolves each guard via the container, erases it to `Arc<dyn Guard>`, and
/// wraps the endpoint with [`nest_rs_guards::GuardExt::guard`] — which calls
/// `Guard::check_http` (the WS upgrade is an HTTP GET) and maps a `Denial` to
/// a poem [`Response`].
///
/// **Dedup against Global**: the WS upgrade is a `Guarded` self-mount —
/// the transport applies the global guard chain at its edge via
/// `SelfMountGuardWrap`, which already runs every global guard's
/// `check_http`. If a gateway-scope `#[use_guards(X)]` matches a TypeId
/// that is also seeded as Global, the wrap is skipped here — same
/// semantics as `RouteShaper` does for HTTP per-route declarations.
fn guard_layers(paths: &[Path]) -> Vec<TokenStream2> {
    paths
        .iter()
        .rev()
        .map(|p| {
            quote! {
                let __ep = {
                    let __type_id = ::core::any::TypeId::of::<#p>();
                    let __is_global = ::nest_rs_core::Container::get::<
                        ::nest_rs_guards::GuardSpecs,
                    >(__container)
                        .is_some_and(|__specs| __specs.0.iter().any(|__s| __s.type_id == __type_id));
                    if __is_global {
                        ::tracing::debug!(
                            target: "nest_rs::layers",
                            layer = ::core::any::type_name::<#p>(),
                            scope = "gateway",
                            "guard declared at multiple scopes — broadest (global) wins, this scope skipped",
                        );
                        __ep
                    } else {
                        ::nest_rs_ws::poem::EndpointExt::boxed(
                            ::nest_rs_ws::poem::EndpointExt::map_to_response(
                            ::nest_rs_guards::GuardExt::guard(
                                __ep,
                                ::nest_rs_core::Container::get::<#p>(__container)
                                    .map(|__arc| __arc as ::std::sync::Arc<dyn ::nest_rs_guards::Guard>)
                                    .expect(concat!(
                                        "#[use_guards] guard `",
                                        stringify!(#p),
                                        "` is not registered — add it to a module's providers"
                                    )),
                            ),
                        ))
                    }
                };
            }
        })
        .collect()
}
