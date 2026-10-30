//! The access-graph entry a host submits when its generated code runs `Repo` —
//! a `#[crud]` host, a route or a field loading a bound subject, the service an
//! exposed entity names for its loaders — so a host reached with no
//! unit-of-work binding fails the boot naming the one to import, instead of
//! every call failing closed at request time.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};

/// `host`'s requirement of the bound unit of work, under `cfgs`.
pub fn unit_of_work_required(host: &impl ToTokens, cfgs: &[TokenStream]) -> TokenStream {
    quote! {
        #(#cfgs)*
        ::nest_rs_core::inventory::submit! {
            ::nest_rs_core::__private::HostRequires {
                host: || ::core::any::TypeId::of::<#host>(),
                requires: ::nest_rs_seaorm::__private::unit_of_work_required,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn the_entry_names_the_host_and_the_seaorm_key_under_its_conditions() {
        let host: syn::Type = parse_quote!(UsersController);
        let entry = unit_of_work_required(&host, &[quote!(#[cfg(feature = "http")])]).to_string();
        assert!(
            entry.starts_with(
                "# [cfg (feature = \"http\")] :: nest_rs_core :: inventory :: submit !"
            ),
            "{entry}"
        );
        assert!(
            entry.contains("TypeId :: of :: < UsersController >"),
            "{entry}"
        );
        assert!(
            entry.contains(":: nest_rs_seaorm :: __private :: unit_of_work_required"),
            "{entry}"
        );
    }
}
