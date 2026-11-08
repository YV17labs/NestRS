//! The `static Site` a provider-hosted impl half — `#[processor]`,
//! `#[scheduled]`, `#[listeners]` — declares for each method it dispatches,
//! from the layers [`take_layers`](crate::layers::take_layers) read on its block
//! and on the method, and the access-graph entry naming those layers.
//!
//! A provider-hosted pair's struct is another decorator's (`#[injectable]`), so
//! the layers a site binds reach the access graph through an inventory entry
//! of their own rather than through the host's `Discoverable`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Type;

use crate::inject::{LayerDeps, layer_deps};
use crate::layers::Layers;
use crate::site::{SiteItems, site_items};
use crate::specs::{exception_filter_specs, interceptor_specs};

/// One dispatch site of a provider-hosted impl half, with the layers its block
/// and its method bound.
pub struct ProviderSite<'a> {
    /// The host's type as the impl block names it.
    pub self_ty: &'a Type,
    /// The dispatched method.
    pub method: &'a syn::Ident,
    /// What the impl block bound, for every method on it.
    pub host: &'a Layers,
    /// What the method bound.
    pub method_layers: &'a Layers,
    /// The method's `#[cfg]` conditions.
    pub cfgs: &'a [TokenStream],
}

/// The hidden associated items declaring `site`, placed in the inherent `impl`
/// of the host that holds its handler const
/// ([`handler_const`](crate::meta::handler_const)), as
/// [`site_items`] writes every site's. Every app-wide layer runs around such a
/// unit: it is accepted on its own, nested in none, and no guard runs.
pub fn site_const(site: &ProviderSite<'_>) -> TokenStream {
    let ProviderSite {
        self_ty,
        method,
        host,
        method_layers,
        cfgs,
    } = site;
    site_items(&SiteItems {
        self_ty,
        method,
        cfgs,
        host_interceptors: interceptor_specs(&host.interceptors),
        host_exception_filters: exception_filter_specs(&host.exception_filters),
        interceptors: &method_layers.interceptors,
        exception_filters: &method_layers.exception_filters,
        global: quote!(::nest_rs_core::__private::GlobalLayers::ALL),
        guards: None,
        class_gate: None,
    })
}

/// The access-graph entry for the layers `site` binds, under its method's
/// `#[cfg]`s; nothing for a site binding none.
pub fn host_layers_entry(site: &ProviderSite<'_>) -> TokenStream {
    let ProviderSite {
        self_ty,
        host,
        method_layers,
        cfgs,
        ..
    } = site;
    let bound = [host, method_layers]
        .into_iter()
        .flat_map(|layers| layers.interceptors.iter().chain(&layers.exception_filters));
    let LayerDeps { keys, labels } = layer_deps(bound);
    if keys.is_empty() {
        return TokenStream::new();
    }
    quote! {
        #(#cfgs)*
        ::nest_rs_core::inventory::submit! {
            ::nest_rs_core::__private::HostLayers {
                host: || ::core::any::TypeId::of::<#self_ty>(),
                layers: || ::std::vec![#((#keys, #labels)),*],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn layers(interceptors: Vec<syn::Path>, exception_filters: Vec<syn::Path>) -> Layers {
        Layers {
            interceptors,
            exception_filters,
            ..Layers::default()
        }
    }

    #[test]
    fn the_block_and_the_method_each_fill_their_own_lists() {
        let ty: Type = parse_quote!(AudioProcessor);
        let method: syn::Ident = parse_quote!(transcode);
        let host = layers(vec![parse_quote!(Audit)], vec![]);
        let method_layers = layers(vec![parse_quote!(Timing)], vec![parse_quote!(Rejected)]);
        let emitted = site_const(&ProviderSite {
            self_ty: &ty,
            method: &method,
            host: &host,
            method_layers: &method_layers,
            cfgs: &[],
        })
        .to_string();
        for part in [
            "host_interceptors : :: std :: vec ! [:: nest_rs_core :: __private :: interceptor :: < Audit > ()]",
            "method_interceptors : :: std :: vec ! [:: nest_rs_core :: __private :: interceptor :: < Timing > ()]",
            "host_exception_filters : :: std :: vec ! []",
            "method_exception_filters : :: std :: vec ! [:: nest_rs_core :: __private :: exception_filter :: < Rejected > ()]",
            "GlobalLayers :: ALL",
            "< AudioProcessor > :: __NESTRS_HANDLER_TRANSCODE",
        ] {
            assert!(emitted.contains(part), "`{part}` in {emitted}");
        }
    }

    #[test]
    fn a_site_binding_no_layer_reports_none_to_the_access_graph() {
        let ty: Type = parse_quote!(AudioProcessor);
        let method: syn::Ident = parse_quote!(transcode);
        let none = Layers::default();
        let site = ProviderSite {
            self_ty: &ty,
            method: &method,
            host: &none,
            method_layers: &none,
            cfgs: &[],
        };
        assert!(host_layers_entry(&site).is_empty());

        let bound = layers(vec![parse_quote!(Audit)], vec![parse_quote!(Rejected)]);
        let entry = host_layers_entry(&ProviderSite {
            host: &bound,
            ..site
        })
        .to_string();
        assert!(entry.contains("HostLayers"), "{entry}");
        assert!(entry.contains("TypeId :: of :: < Audit >"), "{entry}");
        assert!(entry.contains("TypeId :: of :: < Rejected >"), "{entry}");
    }
}
