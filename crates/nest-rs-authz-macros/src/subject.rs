//! `#[subject]` — a unit struct as an authorization subject no table holds.

use nest_rs_codegen::Grammar;
use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt as _;
use syn::{Fields, Item};

/// `#[subject]` takes no argument: a subject is named by its type.
const SUBJECT: Grammar = Grammar::new("subject", &[]);

pub(crate) fn subject(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args, input).unwrap_or_else(syn::Error::into_compile_error)
}

fn expand(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    SUBJECT.parse2(args, |_| Ok(()))?;
    let item = match syn::parse2::<Item>(input)? {
        Item::Struct(item) => item,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "#[subject] goes on a unit struct — `pub struct Exporting;` — the class a rule \
                 grants and `#[authorize]` gates",
            ));
        }
    };
    if !matches!(item.fields, Fields::Unit) {
        return Err(syn::Error::new_spanned(
            &item.fields,
            "#[subject] goes on a unit struct: a subject names a class and holds nothing — a \
             rule grants the whole class, and an entity is the subject whose rows a rule narrows",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[subject] takes no generics: a rule and a posture name one class, by its type",
        ));
    }
    let ident = &item.ident;
    let named = ident.unraw().to_string();
    Ok(quote! {
        #item

        impl ::nest_rs_authz::Subject for #ident {
            const __ROWS: bool = false;

            fn __line_name() -> &'static str {
                #named
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_struct_becomes_a_subject_with_no_rows() {
        let expanded = expand(
            quote!(),
            quote!(
                pub struct Exporting;
            ),
        )
        .expect("a unit struct is a subject")
        .to_string();
        assert!(expanded.contains("pub struct Exporting ;"), "{expanded}");
        assert!(
            expanded.contains("impl :: nest_rs_authz :: Subject for Exporting"),
            "{expanded}"
        );
        assert!(
            expanded.contains("const __ROWS : bool = false"),
            "{expanded}"
        );
    }

    #[test]
    fn every_other_shape_is_refused_naming_the_subject_s() {
        let cases = [
            (
                quote!(),
                quote!(
                    pub struct Exporting {
                        org: u32,
                    }
                ),
                "goes on a unit struct",
            ),
            (
                quote!(),
                quote!(
                    pub struct Exporting(u32);
                ),
                "holds nothing",
            ),
            (
                quote!(),
                quote!(
                    pub struct Exporting<T>;
                ),
                "takes no generics",
            ),
            (
                quote!(),
                quote!(
                    pub enum Exporting {}
                ),
                "goes on a unit struct",
            ),
            (
                quote!(name = "exporting"),
                quote!(
                    pub struct Exporting;
                ),
                "name",
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
