//! The probe an edge's impl half writes beside each dispatched method: every type
//! its answer spells is asked of `nest_rs_core::__private::RowProbe`, so a `Model`
//! no `#[expose]` decorates crosses no wire, whatever holds it and whatever it
//! derives.
//!
//! Spelling only finds the types to ask; whether one is a row is the compiler's
//! answer, after name resolution. A type the probe cannot name where it sits — one
//! naming a lifetime of the method — is skipped and its arguments asked instead.

use proc_macro2::{TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt as _;
use syn::spanned::Spanned as _;
use syn::{GenericArgument, Ident, PathArguments, Type, TypeParamBound};

/// An associated function, compiled and never called, asking of every type
/// `answers` spells whether it is a row; placed in an inherent `impl` of the
/// host, under the method's `cfgs`.
pub fn no_row_answered(method: &Ident, cfgs: &[TokenStream], answers: &[&Type]) -> TokenStream {
    let mut asked = Vec::new();
    for answer in answers {
        spelled_types(answer, &mut asked);
    }
    if asked.is_empty() {
        return TokenStream::new();
    }
    let probe = format_ident!("__nestrs_no_row_{}", method.unraw());
    let calls = asked.iter().map(|ty| {
        // Stable `span()` is a path's first token: point at its own name instead.
        let at = match ty {
            Type::Path(path) => path
                .path
                .segments
                .last()
                .map_or_else(|| ty.span(), |segment| segment.ident.span()),
            _ => ty.span(),
        };
        quote_spanned! {at=>
            ::nest_rs_core::__private::RowProbe::<#ty>::new().answered();
        }
    });
    quote! {
        #(#cfgs)*
        #[doc(hidden)]
        #[allow(dead_code, non_snake_case)]
        fn #probe() {
            #[allow(unused_imports)]
            use ::nest_rs_core::__private::RowProbeFallback as _;
            #(#calls)*
        }
    }
}

/// Every type `ty` spells that a probe can name, outermost first: a path and
/// each type argument it carries, through references, slices, arrays, tuples and
/// the bounds of an `impl Trait` (`impl Stream<Item = Row>` asks of `Row`).
fn spelled_types<'a>(ty: &'a Type, asked: &mut Vec<&'a Type>) {
    match ty {
        Type::Path(path) => {
            if path.qself.is_none() && nameable(ty) {
                asked.push(ty);
            }
            for segment in &path.path.segments {
                arguments(&segment.arguments, asked);
            }
        }
        Type::Reference(reference) => spelled_types(&reference.elem, asked),
        Type::Slice(slice) => spelled_types(&slice.elem, asked),
        Type::Array(array) => spelled_types(&array.elem, asked),
        Type::Paren(paren) => spelled_types(&paren.elem, asked),
        Type::Group(group) => spelled_types(&group.elem, asked),
        Type::Tuple(tuple) => {
            for elem in &tuple.elems {
                spelled_types(elem, asked);
            }
        }
        Type::ImplTrait(bounds) => {
            for bound in &bounds.bounds {
                if let TypeParamBound::Trait(bound) = bound {
                    for segment in &bound.path.segments {
                        arguments(&segment.arguments, asked);
                    }
                }
            }
        }
        _ => {}
    }
}

/// The types a path segment's arguments spell, an associated type's included.
fn arguments<'a>(arguments: &'a PathArguments, asked: &mut Vec<&'a Type>) {
    let PathArguments::AngleBracketed(arguments) = arguments else {
        return;
    };
    for argument in &arguments.args {
        match argument {
            GenericArgument::Type(ty) => spelled_types(ty, asked),
            GenericArgument::AssocType(assoc) => spelled_types(&assoc.ty, asked),
            _ => {}
        }
    }
}

/// Whether `ty` can be written in a function of its own: it names no lifetime
/// but `'static`, infers nothing and holds no `impl` or `dyn`.
fn nameable(ty: &Type) -> bool {
    fn walk(tokens: TokenStream) -> bool {
        let mut after_apostrophe = false;
        for token in tokens {
            let lifetime = std::mem::take(&mut after_apostrophe);
            match token {
                TokenTree::Group(group) if !walk(group.stream()) => return false,
                TokenTree::Punct(punct) => after_apostrophe = punct.as_char() == '\'',
                TokenTree::Ident(ident)
                    if (lifetime && ident != "static")
                        || ident == "_"
                        || ident == "impl"
                        || ident == "dyn" =>
                {
                    return false;
                }
                _ => {}
            }
        }
        true
    }
    walk(quote!(#ty))
}

#[cfg(test)]
mod tests {
    use quote::{format_ident, quote};
    use syn::parse_quote;

    use super::*;

    fn asked(ty: &Type) -> Vec<String> {
        let mut asked = Vec::new();
        spelled_types(ty, &mut asked);
        asked
            .into_iter()
            .map(|ty| quote!(#ty).to_string().replace(' ', ""))
            .collect()
    }

    #[test]
    fn every_type_an_answer_spells_is_asked() {
        let ty: Type = parse_quote!(Result<Json<Vec<secret::Model>>, HttpError>);
        assert_eq!(
            asked(&ty),
            [
                "Result<Json<Vec<secret::Model>>,HttpError>",
                "Json<Vec<secret::Model>>",
                "Vec<secret::Model>",
                "secret::Model",
                "HttpError",
            ]
        );
    }

    #[test]
    fn a_stream_asks_of_its_items() {
        let ty: Type = parse_quote!(impl Stream<Item = Result<secret::Model, E>> + Send);
        assert_eq!(
            asked(&ty),
            ["Result<secret::Model,E>", "secret::Model", "E"]
        );
    }

    #[test]
    fn a_type_naming_a_lifetime_asks_of_its_arguments_alone() {
        let ty: Type = parse_quote!(Cow<'a, [secret::Model]>);
        assert_eq!(asked(&ty), ["secret::Model"]);
        let ty: Type = parse_quote!(&'static str);
        assert_eq!(asked(&ty), ["str"]);
    }

    #[test]
    fn a_raw_method_name_names_its_probe() {
        let row: Type = parse_quote!(secret::Model);
        let probe = no_row_answered(&format_ident!("r#type"), &[], &[&row]).to_string();
        assert!(probe.contains("__nestrs_no_row_type"), "{probe}");
    }

    #[test]
    fn nothing_to_ask_writes_nothing() {
        let unit: Type = parse_quote!(());
        assert!(no_row_answered(&format_ident!("ping"), &[], &[&unit]).is_empty());
    }
}
