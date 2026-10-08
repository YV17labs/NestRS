//! `#[gateway]` — struct decorator: construction, `PATH`/`VERSION` and the
//! connection-level guard wrapping `#[messages]` reads.

use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{LitStr, Path};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, from_container_method, guard_capability_bounds,
    injected_keys_with_layers, injected_names_with_layers, layer_deps, reject_http_only_layers,
    scoped_specs, take_path_list,
};

pub(crate) fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    let GatewayArgs {
        path,
        version,
        namespace,
    } = match parse_gateway_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    let path_lit = path;
    let version_opt = match &version {
        Some(v) => quote! { ::core::option::Option::Some(#v) },
        None => quote! { ::core::option::Option::None },
    };
    let mut item = match pair::WS.parse_host(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };

    if let Err(err) = reject_http_only_layers(&item.attrs, "WebSockets", "gateway") {
        return err.to_compile_error().into();
    }

    let guards = match take_path_list(&mut item.attrs, "use_guards") {
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
    let layers = layer_deps(guards.iter());
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);

    let guard_layers = guard_layers(&guards);
    let has_edge_guards = !guards.is_empty();
    let edge_guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    // The upgrade is an HTTP `GET`: a gateway-scope guard attests `HttpGuard`,
    // not the `WsGuard` per-event ones owe.
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::HttpGuard));

    let ns_ty = match &namespace {
        Some(path) => quote! { #path },
        None => quote! { ::nest_rs_ws::Global },
    };
    // Declared to the access graph so a missing `WsModule` fails boot by name
    // rather than panicking at mount.
    let registry_key = quote! { ::core::any::TypeId::of::<::nest_rs_ws::WsServer<#ns_ty>>() };
    // What a missing import prints; the docs quote the bare `WsServer` verbatim.
    let registry_label: &str = &match &namespace {
        Some(path) => format!(
            "WsServer<{}>",
            path.segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default(),
        ),
        None => "WsServer".to_owned(),
    };

    // Submitted to the registry `WsNamespaces` drains, so `WsModule` owns every
    // `WsServer<N>` and the access graph can name it.
    let namespace_submission = match &namespace {
        Some(_) => quote! {
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_ws::WsNamespaceEntry {
                    key: || ::core::any::TypeId::of::<::nest_rs_ws::WsServer<#ns_ty>>(),
                    label: #registry_label,
                    provide: |__builder| ::nest_rs_core::ContainerBuilder::provide(
                        __builder,
                        <::nest_rs_ws::WsServer<#ns_ty>>::default(),
                    ),
                }
            }
        },
        None => quote! {},
    };

    let residency = pair::WS.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        #namespace_submission

        impl #impl_generics #name #ty_generics #where_clause {
            pub const PATH: &'static str = #path_lit;

            /// The URI version segment from `#[gateway(version = "…")]`, if any.
            pub const VERSION: ::core::option::Option<&'static str> = #version_opt;

            /// The address a client connects to — `PATH` with the version folded in.
            #[doc(hidden)]
            pub fn __nestrs_mount_path() -> ::std::string::String {
                ::nest_rs_ws::nest_rs_http::version_path(Self::VERSION, Self::PATH)
            }

            /// Whether this gateway binds its own guards at the upgrade, which the
            /// transport cannot see inside the mount closure.
            #[doc(hidden)]
            pub const HAS_EDGE_GUARDS: bool = #has_edge_guards;

            /// The upgrade-scope guards, as the specs the boot check reads.
            #[doc(hidden)]
            pub fn __nestrs_edge_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                #edge_guard_specs
            }

            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                let mut __keys = #injected_keys;
                __keys.push(#registry_key);
                __keys
            }

            #[doc(hidden)]
            pub fn __nestrs_injected_names() -> ::std::vec::Vec<&'static str> {
                let mut __names = #injected_names;
                __names.push(#registry_label);
                __names
            }

            #[doc(hidden)]
            pub fn __nestrs_registry(
                __container: &::nest_rs_core::Container,
            ) -> ::std::sync::Arc<::nest_rs_ws::WsServer<#ns_ty>> {
                // Unreachable once booted: the access graph refuses a missing registry.
                ::nest_rs_core::Container::get::<::nest_rs_ws::WsServer<#ns_ty>>(__container).expect(
                    "WebSocket gateway requires its connection registry — add `WsModule` to a \
                     module's `imports`; it owns every registry, namespaced or not",
                )
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
    version: Option<LitStr>,
    namespace: Option<Path>,
}

const GATEWAY: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("gateway", &["path", "version", "namespace"]).with_remedies(&[(
        "version",
        "a gateway owns one mount, so it carries one version",
    )]);

/// Parse `#[gateway(path = "/ws", version = "1", namespace = ChatNs)]`; `path`
/// is required.
fn parse_gateway_args(args: TokenStream2) -> syn::Result<GatewayArgs> {
    let mut path = None;
    let mut version = None;
    let mut namespace = None;
    GATEWAY.parse2(args, |arg| {
        match arg.key() {
            "path" => path = Some(arg.str_lit("/ws")?),
            // `#[controller]`'s parser, so the version grammar is one.
            "version" => {
                let value = arg.expr()?;
                let declared =
                    nest_rs_codegen::versioning::parse_version_list(&value, "#[gateway]")?;
                if declared.len() > 1 {
                    return Err(syn::Error::new_spanned(
                        &value,
                        nest_rs_codegen::takes_value(
                            "gateway",
                            Some("version"),
                            "one version: a gateway owns its mount outright, so there is no \
                             second path for a second version to answer at — declare one \
                             gateway per version",
                        ),
                    ));
                }
                version = declared.into_iter().next();
            }
            "namespace" => namespace = Some(expr_path(&arg.expr()?)?),
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            nest_rs_codegen::missing_argument("gateway", "path", "\"/ws\""),
        )
    })?;
    nest_rs_codegen::reject_path("gateway", &path)?;
    Ok(GatewayArgs {
        path,
        version,
        namespace,
    })
}

/// `namespace = ChatNs`'s value, read through the invisible group a
/// `macro_rules!` forwards it in.
fn expr_path(expr: &syn::Expr) -> syn::Result<Path> {
    match nest_rs_codegen::ungrouped_expr(expr) {
        syn::Expr::Path(p) => Ok(p.path.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            nest_rs_codegen::takes_value(
                "gateway",
                Some("namespace"),
                "a marker type path, e.g. `namespace = ChatNs`",
            ),
        )),
    }
}

/// The connection-level guard layers, reversed so the first-listed guard ends
/// up outermost. A guard also seeded globally is skipped: the transport's
/// `SelfMountGuardWrap` already runs it at the edge.
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
                        ::nest_rs_ws::tracing::debug!(
                            target: ::nest_rs_ws::target::LAYERS,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: proc_macro2::TokenStream) -> syn::Result<GatewayArgs> {
        parse_gateway_args(args)
    }

    /// `GatewayArgs` holds `syn` types with no `Debug`, so `expect_err` is out.
    fn refusal(args: proc_macro2::TokenStream) -> String {
        match parse_gateway_args(args) {
            Ok(args) => panic!(
                "expected a refusal, parsed `path = {:?}`",
                args.path.value()
            ),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn version_is_optional_and_absent_by_default() {
        let args = parse(quote! { path = "/ws" }).expect("path alone is the minimal form");
        assert_eq!(args.path.value(), "/ws");
        assert!(
            args.version.is_none(),
            "an undeclared version stays `None`, so `version_path` leaves the mount alone",
        );
    }

    #[test]
    fn all_three_keys_parse_in_any_order() {
        let args = parse(quote! { version = "1", namespace = ChatNs, path = "/ws" })
            .expect("the keys are order-independent, like #[controller]'s");
        assert_eq!(args.path.value(), "/ws");
        assert_eq!(args.version.map(|v| v.value()).as_deref(), Some("1"));
        assert!(args.namespace.is_some());
    }

    #[test]
    fn a_version_is_any_string_token() {
        let args = parse(quote! { path = "/ws", version = "2024-08-11" }).expect("a date version");
        assert_eq!(
            args.version.map(|v| v.value()).as_deref(),
            Some("2024-08-11"),
        );
    }

    #[test]
    fn an_unknown_key_names_every_accepted_one() {
        let message = refusal(quote! { path = "/ws", prefix = "/v1" });
        for key in ["path", "version", "namespace"] {
            assert!(
                message.contains(key),
                "the diagnostic must name `{key}`, so a developer learns the whole grammar from \
                 one error: {message}",
            );
        }
    }

    #[test]
    fn a_version_without_a_path_is_still_refused() {
        let message = refusal(quote! { version = "1" });
        assert!(
            message.contains("path"),
            "a version is an optional refinement of an address, never the address: {message}",
        );
    }
}
