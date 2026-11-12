//! The hidden items every decorator dispatching a unit declares for its site,
//! in the host's inherent `impl`: the function building its interceptors and
//! exception filters, the one building its guard stage where the edge has
//! one, and the const holding its `&'static Site`, which [`site_ref`] reads.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Path, Type};

use crate::meta::{handler_ref, site_const_ident, site_fn_ident};
use crate::specs::{exception_filter_specs, interceptor_specs};

/// One dispatch site, as its decorator declares it.
pub struct SiteItems<'a> {
    /// The host's type as the impl block names it.
    pub self_ty: &'a Type,
    /// The dispatched method, which names every item.
    pub method: &'a syn::Ident,
    /// The method's `#[cfg]` conditions, on every item.
    pub cfgs: &'a [TokenStream],
    /// The interceptors the host binds around each of its units, a `Vec`
    /// expression.
    pub host_interceptors: TokenStream,
    /// The exception filters the host binds around each of its units, a `Vec`
    /// expression.
    pub host_exception_filters: TokenStream,
    /// The interceptors the method binds.
    pub interceptors: &'a [Path],
    /// The exception filters the method binds.
    pub exception_filters: &'a [Path],
    /// The app-wide layers around the unit, a `GlobalLayers` expression.
    pub global: TokenStream,
    /// The guard stage, for an edge whose units run one.
    pub guards: Option<SiteGuards>,
    /// The posture's class gate, a `ClassGate` expression, where it declares
    /// one.
    pub class_gate: Option<TokenStream>,
}

/// The call building a site's guard stage:
/// `build(container, handler, declaration)`.
pub struct SiteGuards {
    /// The edge's builder, `::nest_rs_<edge>::__private::<unit>_guard_stage`.
    pub build: TokenStream,
    /// What the builder reads, the edge's declaration literal.
    pub declaration: TokenStream,
}

/// The items declaring `site`: every one hidden, under the method's
/// `#[cfg]`s.
pub fn site_items(site: &SiteItems<'_>) -> TokenStream {
    let SiteItems {
        self_ty,
        method,
        cfgs,
        host_interceptors,
        host_exception_filters,
        interceptors,
        exception_filters,
        global,
        guards,
        class_gate,
    } = site;
    let name = site_const_ident("site", method);
    let layers_fn = site_fn_ident("layers", method);
    let handler = handler_ref(self_ty, method);
    let method_interceptors = interceptor_specs(interceptors);
    let method_exception_filters = exception_filter_specs(exception_filters);
    let (guards_fn, guard_stage) = match guards {
        Some(SiteGuards { build, declaration }) => {
            let guards_fn = site_fn_ident("guards", method);
            (
                quote! {
                    #(#cfgs)*
                    #[doc(hidden)]
                    fn #guards_fn(
                        __nestrs_container: &::nest_rs_core::Container,
                        __nestrs_handler: &::nest_rs_core::Handler,
                    ) -> ::nest_rs_core::anyhow::Result<
                        ::core::option::Option<
                            ::std::sync::Arc<dyn ::nest_rs_core::__private::GuardStage>,
                        >,
                    > {
                        #build(__nestrs_container, __nestrs_handler, #declaration)
                    }
                },
                quote! {
                    ::core::option::Option::Some(
                        <#self_ty>::#guards_fn as ::nest_rs_core::__private::GuardStageFactory,
                    )
                },
            )
        }
        None => (TokenStream::new(), quote!(::core::option::Option::None)),
    };
    let class_gate = match class_gate {
        Some(gate) => quote!(::core::option::Option::Some(#gate)),
        None => quote!(::core::option::Option::None),
    };
    quote! {
        #(#cfgs)*
        #[doc(hidden)]
        fn #layers_fn() -> ::nest_rs_core::__private::SiteLayers {
            ::nest_rs_core::__private::SiteLayers {
                host_interceptors: #host_interceptors,
                method_interceptors: #method_interceptors,
                host_exception_filters: #host_exception_filters,
                method_exception_filters: #method_exception_filters,
            }
        }

        #guards_fn

        #(#cfgs)*
        #[doc(hidden)]
        pub const #name: &'static ::nest_rs_core::__private::Site = {
            static __NESTRS_SITE: ::nest_rs_core::__private::Site =
                ::nest_rs_core::__private::declare_site(
                    #handler,
                    ::nest_rs_core::__private::SiteDeclaration {
                        layers: <#self_ty>::#layers_fn,
                        global: #global,
                        guard_stage: #guard_stage,
                        class_gate: #class_gate,
                    },
                );
            &__NESTRS_SITE
        };
    }
}

/// The `&'static Site` of `method`'s site on `self_ty`.
pub fn site_ref(self_ty: &Type, method: &syn::Ident) -> TokenStream {
    let name = site_const_ident("site", method);
    quote! { <#self_ty>::#name }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn site<'a>(
        self_ty: &'a Type,
        method: &'a syn::Ident,
        interceptors: &'a [Path],
        guards: Option<SiteGuards>,
    ) -> SiteItems<'a> {
        SiteItems {
            self_ty,
            method,
            cfgs: &[],
            host_interceptors: quote!(Host::interceptors()),
            host_exception_filters: quote!(Host::filters()),
            interceptors,
            exception_filters: &[],
            global: quote!(GLOBAL),
            guards,
            class_gate: None,
        }
    }

    #[test]
    fn a_site_const_is_named_for_its_method() {
        let ty: Type = parse_quote!(AudioProcessor);
        let method: syn::Ident = parse_quote!(r#type);
        assert_eq!(
            site_ref(&ty, &method).to_string(),
            "< AudioProcessor > :: __NESTRS_SITE_TYPE",
        );
    }

    #[test]
    fn the_host_and_the_method_each_fill_their_own_lists() {
        let ty: Type = parse_quote!(Chat);
        let method: syn::Ident = parse_quote!(send);
        let timing: [Path; 1] = [parse_quote!(Timing)];
        let emitted = site_items(&site(&ty, &method, &timing, None)).to_string();
        for part in [
            "host_interceptors : Host :: interceptors ()",
            "method_interceptors : :: std :: vec ! [:: nest_rs_core :: __private :: interceptor :: < Timing > ()]",
            "host_exception_filters : Host :: filters ()",
            "method_exception_filters : :: std :: vec ! []",
            "global : GLOBAL",
            "layers : < Chat > :: __nestrs_layers_send",
            "< Chat > :: __NESTRS_HANDLER_SEND",
        ] {
            assert!(emitted.contains(part), "`{part}` in {emitted}");
        }
    }

    #[test]
    fn a_site_without_guards_declares_no_stage_and_one_with_them_builds_it() {
        let ty: Type = parse_quote!(Chat);
        let method: syn::Ident = parse_quote!(send);
        let bare = site_items(&site(&ty, &method, &[], None)).to_string();
        assert!(!bare.contains("__nestrs_guards_send"), "{bare}");
        assert!(
            bare.contains("guard_stage : :: core :: option :: Option :: None"),
            "{bare}"
        );

        let guarded = site_items(&site(
            &ty,
            &method,
            &[],
            Some(SiteGuards {
                build: quote!(edge::stage),
                declaration: quote!(Declared {}),
            }),
        ))
        .to_string();
        for part in [
            "fn __nestrs_guards_send (__nestrs_container",
            "edge :: stage (__nestrs_container , __nestrs_handler , Declared { })",
            "< Chat > :: __nestrs_guards_send as :: nest_rs_core :: __private :: GuardStageFactory",
        ] {
            assert!(guarded.contains(part), "`{part}` in {guarded}");
        }
    }
}
