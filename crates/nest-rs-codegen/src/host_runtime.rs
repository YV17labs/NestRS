//! The entry a decorator submits for the provider it makes an edge's host, so
//! the `#[module]` listing it imports what the edge runs on.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};

/// `host`'s entry naming `runtime`, the modules its edge's crate declares,
/// under `cfgs`.
pub fn host_runtime_entry(
    host: &impl ToTokens,
    runtime: &TokenStream,
    cfgs: &[TokenStream],
) -> TokenStream {
    quote! {
        #(#cfgs)*
        ::nest_rs_core::inventory::submit! {
            ::nest_rs_core::__private::HostRuntime {
                host: || ::core::any::TypeId::of::<#host>(),
                name: ::core::any::type_name::<#host>,
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
            entry.contains("modules : :: nest_rs_queue :: __private :: RUNTIME"),
            "{entry}"
        );
    }
}
