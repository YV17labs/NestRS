//! Layer-System token emission shared by the transport decorators: every
//! scope's `#[use_*(...)]` paths become one `Vec<ScopedLayerSpec>` shape.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Path;

/// `Vec<ScopedLayerSpec>` for a scope's layer paths, each entry capturing the
/// type id, name and a container-resolve closure that coerces the provider to
/// `erased` (e.g. `dyn ::nest_rs_guards::Guard`). Empty paths ⇒ `::std::vec![]`.
pub fn scoped_specs(paths: &[Path], erased: TokenStream2) -> TokenStream2 {
    let entries = paths.iter().map(|p| {
        quote! {
            ::nest_rs_guards::dispatch::ScopedLayerSpec::new(
                ::core::any::TypeId::of::<#p>(),
                ::core::any::type_name::<#p>(),
                |__c| ::nest_rs_core::Container::get::<#p>(__c)
                    .map(|__arc| __arc as ::std::sync::Arc<#erased>),
            )
        }
    });
    quote! { ::std::vec![#(#entries),*] }
}

/// `Vec<TypeId>` for `#[force_guards(...)]` — the Layer-System opt-in that lets
/// a scoped guard re-run even when its `TypeId` is already in a broader chain.
pub fn force_guard_typeids(paths: &[Path]) -> TokenStream2 {
    let entries = paths
        .iter()
        .map(|p| quote! { ::core::any::TypeId::of::<#p>() });
    quote! { ::std::vec![#(#entries),*] }
}
