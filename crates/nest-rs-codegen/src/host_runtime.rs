//! The entry a decorator submits for the provider it makes an edge's host, so
//! the `#[module]` listing it imports what the edge runs on.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};

/// How an edge finds its hosts, as its entry records it.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum HostMount {
    /// It mounts what the host's `Discoverable::register` attached.
    Attached,
    /// It reads the link-time registry against what the app reaches.
    Linked,
}

/// `host`'s entry naming `runtime`, the modules its edge's crate declares, and
/// how the edge finds it, under `cfgs`.
pub fn host_runtime_entry(
    host: &impl ToTokens,
    runtime: &TokenStream,
    mount: HostMount,
    cfgs: &[TokenStream],
) -> TokenStream {
    let mount = match mount {
        HostMount::Attached => quote!(Attached),
        HostMount::Linked => quote!(Linked),
    };
    quote! {
        #(#cfgs)*
        ::nest_rs_core::inventory::submit! {
            ::nest_rs_core::__private::HostRuntime {
                host: || ::core::any::TypeId::of::<#host>(),
                name: ::core::any::type_name::<#host>,
                mount: ::nest_rs_core::__private::HostMount::#mount,
                modules: #runtime,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn the_entry_names_the_host_and_the_edges_modules_under_its_conditions() {
        let host: syn::Type = parse_quote!(ReportsProcessor);
        let entry = host_runtime_entry(
            &host,
            &quote!(::nest_rs_queue::__private::RUNTIME),
            HostMount::Linked,
            &[quote!(#[cfg(feature = "jobs")])],
        )
        .to_string();
        assert!(
            entry.starts_with(
                "# [cfg (feature = \"jobs\")] :: nest_rs_core :: inventory :: submit !"
            ),
            "{entry}"
        );
        assert!(
            entry.contains("TypeId :: of :: < ReportsProcessor >"),
            "{entry}"
        );
        assert!(
            entry.contains("name : :: core :: any :: type_name :: < ReportsProcessor >"),
            "{entry}"
        );
        assert!(
            entry.contains("mount : :: nest_rs_core :: __private :: HostMount :: Linked"),
            "{entry}"
        );
        assert!(
            entry.contains("modules : :: nest_rs_queue :: __private :: RUNTIME"),
            "{entry}"
        );
    }
}
