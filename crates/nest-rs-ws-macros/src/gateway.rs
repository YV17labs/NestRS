//! `#[gateway]` — struct decorator (construction + `PATH`/`VERSION` consts +
//! connection-level guard wrapping). `#[messages]` emits the `Discoverable`/mount
//! + dispatcher, and reads the mount address back from here.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{LitStr, Meta, Path, Token};

use nest_rs_codegen::{
    DecoratorPair, InjectableBody, build_injectable_body, expr_str, from_container_method,
    guard_capability_bounds, injected_keys_with_layers, injected_names_with_layers, layer_deps,
    reject_http_only_layers, scoped_specs, take_path_list,
};

/// The WS edge's pair, read by both halves so their wrong-shape diagnostics name
/// each other rather than reporting syn's `expected struct`.
pub(crate) const WS_PAIR: DecoratorPair = DecoratorPair {
    host: "#[gateway]",
    subject: "gateway struct",
    operations: "#[messages]",
    collects: "#[subscribe_message] / #[on_connect] / #[on_disconnect]",
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
    let mut item = match WS_PAIR.parse_host(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };

    if let Err(err) = reject_http_only_layers(&item.attrs, "WebSockets", "gateway") {
        return err.to_compile_error().into();
    }

    // `@UseGuards` analog on the struct — run on the WS upgrade.
    let guards = match take_path_list(&mut item.attrs, "use_guards", "entry") {
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
    // Access-graph deps: `#[inject]` keys + connection-level guards. Exposed
    // through an inherent fn `#[messages]` reads back (and extends with its
    // per-message guards) when emitting `Discoverable::injected`.
    // Keys plus index-aligned labels from one walk, so a connection guard no
    // module provides is named in the boot error rather than reported as
    // `<unnamed dependency>`.
    let layers = layer_deps(guards.iter());
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);

    // Connection-level guard layers; first listed ends up outermost. With
    // nothing declared this just boxes the endpoint.
    let guard_layers = guard_layers(&guards);
    let has_edge_guards = !guards.is_empty();
    let edge_guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    // A gateway-scope guard runs on the upgrade, which is an HTTP `GET`, so it
    // attests `HttpGuard` — not `WsGuard`, which `#[messages]` requires of the
    // per-event ones. That split is the whole point of the two markers here.
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::HttpGuard));

    let ns_ty = match &namespace {
        Some(path) => quote! { #path },
        None => quote! { ::nest_rs_ws::Global },
    };
    // The connection registry is a hard dependency of every gateway — the
    // `WsClient` each handler receives reads it — so it belongs under the
    // access contract like any `#[inject]` field. Declared here, a missing
    // `WsModule` is a clean boot error naming the module that provides it
    // (`AccessGraphError`), instead of a mount-time panic with a backtrace
    // note: the app used to compile, log `mounted endpoint kind="ws"`, and
    // *then* die. Namespaced or not, the registry comes from `WsModule`, so the
    // same declaration and the same diagnostic cover both.
    let registry_key = quote! { ::core::any::TypeId::of::<::nest_rs_ws::WsServer<#ns_ty>>() };
    // Names the *key*, marker included, because this label is what a missing
    // import prints. `WsServer` for the default namespace keeps the diagnostic
    // the docs quote verbatim; `WsServer<NotifyNs>` distinguishes the rest, which
    // is the whole point of having several.
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

    // A namespaced gateway submits its marker to the link-time registry
    // `WsNamespaces` (a provider of `WsModule`) drains, so `WsModule` owns every
    // registry — namespaced or `Global` — and the access graph can name it. The
    // gateway used to install its own from `Discoverable::register`, which left
    // the key belonging to no module (so the graph waved consumers through) and
    // made construction order depend on where the gateway sat in the tree.
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

    let residency = WS_PAIR.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        #namespace_submission

        impl #impl_generics #name #ty_generics #where_clause {
            pub const PATH: &'static str = #path_lit;

            /// The URI version segment from `#[gateway(version = "…")]`; `None`
            /// when the gateway declares none. Same const `#[controller]` emits,
            /// because a gateway's mount is an address a client selects.
            pub const VERSION: ::core::option::Option<&'static str> = #version_opt;

            /// The address a client actually connects to — `PATH` with the
            /// declared version folded in. Built by `nest_rs_http::version_path`,
            /// the single place URI versioning lives, so a gateway's served path
            /// and a controller's cannot drift.
            ///
            /// Read by `#[messages]` three times over — the `HttpEndpointMeta`
            /// path (which is what the duplicate-mount boot check compares), the
            /// `Route::at` it mounts at, and the boot log — so those three cannot
            /// disagree either.
            #[doc(hidden)]
            pub fn __nestrs_mount_path() -> ::std::string::String {
                ::nest_rs_ws::nest_rs_http::version_path(Self::VERSION, Self::PATH)
            }

            /// Whether this gateway binds its own guards at the upgrade. The
            /// transport cannot see inside the mount closure, so it reads this
            /// to tell a guarded edge from a bare one instead of warning on both.
            #[doc(hidden)]
            pub const HAS_EDGE_GUARDS: bool = #has_edge_guards;

            /// The upgrade-scope guards, as resolvable specs.
            ///
            /// The layers above *apply* them; this is the same list in the shape
            /// the boot check reads, so `#[messages]` can phase-validate the
            /// upgrade chain the way `#[routes]` validates a controller's. The
            /// two halves of the pair are one decorator's worth of information
            /// split across two item shapes, and this is the seam between them.
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
                // Unreachable in a booted app: the registry is declared in
                // `__nestrs_injected`, so the access graph refuses the boot
                // before any mount runs. Kept as a defensive resolve for a
                // hand-assembled container that skips that check.
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

/// Parse `#[gateway(path = "/ws", version = "1", namespace = ChatNs)]` — `path`
/// required, the other two optional. Order-independent; unknown keys rejected.
///
/// `version` is spelled and parsed exactly as `#[controller]`'s, deliberately: a
/// gateway's mount is a path a client selects, so the declaration a developer
/// writes for it must not be a second dialect of the one they already know.
fn parse_gateway_args(args: TokenStream2) -> syn::Result<GatewayArgs> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut path = None;
    let mut version = None;
    let mut namespace = None;
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => {
                if path.is_some() {
                    return Err(syn::Error::new_spanned(
                        &nv,
                        "#[gateway] declares `path` twice — keep one",
                    ));
                }
                path = Some(expr_str(&nv.value)?);
            }
            // Through `#[controller]`'s own parser, which is what the doc
            // comment above already claims. Taking it through a bare
            // `expr_str` made this a *second* grammar wearing one name: no
            // character-set check, so `version = "a/b"` compiled and mounted at
            // `/va/b/ws`, and no list form, so the spelling learned next door
            // failed here with a bare "expected a string literal".
            Meta::NameValue(nv) if nv.path.is_ident("version") => {
                if version.is_some() {
                    return Err(syn::Error::new_spanned(
                        &nv,
                        "#[gateway] declares `version` twice — a gateway owns one mount, so it \
                         carries one version",
                    ));
                }
                let declared =
                    nest_rs_codegen::versioning::parse_version_list(&nv.value, "#[gateway]")?;
                if declared.len() > 1 {
                    return Err(syn::Error::new_spanned(
                        &nv.value,
                        "#[gateway] serves one version: a gateway owns its mount outright, so \
                         there is no second path for a second version to answer at — declare one \
                         gateway per version",
                    ));
                }
                version = declared.into_iter().next();
            }
            Meta::NameValue(nv) if nv.path.is_ident("namespace") => {
                namespace = Some(expr_path(&nv.value)?)
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[gateway] accepts `path = \"...\"`, an optional `version = \"...\"` and an \
                     optional `namespace = MarkerType`",
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
    Ok(GatewayArgs {
        path,
        version,
        namespace,
    })
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
                        ::nest_rs_ws::tracing::debug!(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: proc_macro2::TokenStream) -> syn::Result<GatewayArgs> {
        parse_gateway_args(args)
    }

    /// `GatewayArgs` holds `syn` types with no `Debug`, so `expect_err` is out;
    /// the refusal's wording is what these tests are about anyway.
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

    /// A version is an opaque token, not a number: `#[controller(version =
    /// "2024-08-11")]` is legal, so the gateway's half must not narrow it.
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
