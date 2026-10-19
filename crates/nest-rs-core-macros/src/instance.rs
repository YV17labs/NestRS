//! `#[instance]` — a unit struct as an `Instance` marker, named by its ident.

use nest_rs_codegen::__private::{INSTANCE, instance_name};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, Item};

pub(crate) fn instance(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args, input).unwrap_or_else(syn::Error::into_compile_error)
}

fn expand(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    INSTANCE.parse2(args, |_| Ok(()))?;
    let item = match syn::parse2::<Item>(input)? {
        Item::Struct(item) => item,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "#[instance] goes on a unit struct — `pub struct Cache;` — the marker naming one \
                 instance of a resource",
            ));
        }
    };
    if !matches!(item.fields, Fields::Unit) {
        return Err(syn::Error::new_spanned(
            &item.fields,
            "#[instance] goes on a unit struct: a marker names an instance and holds nothing — \
             the instance's values are its config's and its resource's",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[instance] takes no generics: a marker names one instance, read off its ident",
        ));
    }
    let ident = &item.ident;
    let name = instance_name(ident).map_err(|refusal| syn::Error::new_spanned(ident, refusal))?;
    let declared = syn::ext::IdentExt::unraw(ident).to_string();
    Ok(quote! {
        #item

        impl ::nest_rs_core::__private::InstanceSeal for #ident {}

        impl ::nest_rs_core::Instance for #ident {
            const NAME: &'static str = #name;
            type Of<__T: ::core::marker::Send + ::core::marker::Sync + 'static> =
                ::nest_rs_core::Instanced<Self, __T>;
        }

        ::nest_rs_core::inventory::submit! {
            ::nest_rs_core::__private::InstanceDeclaration::new(
                #name,
                ::core::concat!(::core::module_path!(), "::", #declared),
                ::core::any::TypeId::of::<#ident>,
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_struct_reads_its_name_and_declares_it() {
        let item = quote!(
            pub struct ReadReplica;
        );
        let expanded = expand(quote!(), item)
            .expect("a unit struct is a marker")
            .to_string();
        assert!(expanded.contains("\"read_replica\""), "{expanded}");
        assert!(expanded.contains("InstanceDeclaration"), "{expanded}");
    }

    #[test]
    fn every_other_shape_is_refused_naming_the_marker_s() {
        let cases = [
            (
                quote!(),
                quote!(
                    pub struct Cache {
                        url: String,
                    }
                ),
                "goes on a unit struct",
            ),
            (
                quote!(),
                quote!(
                    pub struct Cache<T>;
                ),
                "takes no generics",
            ),
            (
                quote!(),
                quote!(
                    pub enum Cache {}
                ),
                "goes on a unit struct",
            ),
            (
                quote!(name = "cache"),
                quote!(
                    pub struct Cache;
                ),
                "rename the struct",
            ),
        ];
        for (args, item, says) in cases {
            let refused = match expand(args, item) {
                Ok(_) => panic!("refused: {says}"),
                Err(refusal) => refusal.to_string(),
            };
            assert!(refused.contains(says), "{refused}");
        }
    }
}
