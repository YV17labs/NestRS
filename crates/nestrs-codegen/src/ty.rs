//! Type/path inspection helpers shared by the decorator macros.

use quote::quote;
use syn::{GenericArgument, Ident, PathArguments, Type, TypeParamBound};

/// The base ident of an impl block's self type — last path segment of
/// `impl Foo` / `impl path::to::Foo`. Errors on a non-path self type;
/// `decorator` names the caller for the error.
pub fn impl_self_ident(self_ty: &Type, decorator: &str) -> syn::Result<Ident> {
    match self_ty {
        Type::Path(tp) => tp.path.segments.last().map(|seg| seg.ident.clone()),
        _ => None,
    }
    .ok_or_else(|| {
        syn::Error::new_spanned(
            self_ty,
            format!("{decorator} requires a simple struct path (e.g. `impl MyService`)"),
        )
    })
}

/// If `ty` syntactically matches `Arc<Inner>`, return `Inner`. Inspects only
/// the last path segment, so `std::sync::Arc<T>` works as well as `Arc<T>`.
pub(crate) fn arc_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Arc" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    let GenericArgument::Type(inner) = &args.args[0] else {
        return None;
    };
    Some(inner)
}

/// Short label for a dependency type in diagnostics: last path segment, or
/// `dyn Trait` for a trait object, or the token rendering otherwise.
pub(crate) fn type_label(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
            .unwrap_or_else(|| quote!(#ty).to_string()),
        Type::TraitObject(to) => {
            let trait_name = to.bounds.iter().find_map(|b| match b {
                TypeParamBound::Trait(t) => t.path.segments.last().map(|seg| seg.ident.to_string()),
                _ => None,
            });
            match trait_name {
                Some(name) => format!("dyn {name}"),
                None => quote!(#ty).to_string(),
            }
        }
        _ => quote!(#ty).to_string(),
    }
}

/// The `idx`-th generic type argument of `ty` when its last segment is
/// `name<...>` — peels a transport wrapper (`Json`, `Result`, `Valid`,
/// `Piped`) off a payload type.
pub fn nth_generic_type<'a>(ty: &'a Type, name: &str, idx: usize) -> Option<&'a Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != name {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args
        .iter()
        .filter_map(|arg| match arg {
            GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .nth(idx)
}
