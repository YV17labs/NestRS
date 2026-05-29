//! Type/path inspection helpers shared by the decorator macros.

use quote::quote;
use syn::{GenericArgument, Ident, PathArguments, Type, TypeParamBound};

/// The base ident of an impl block's self type — the last path segment of
/// `impl Foo` / `impl path::to::Foo`. The impl-block decorators (`#[routes]`,
/// `#[resolver]`, `#[dataloader]`, `#[hooks]`) need it to name generated items
/// and to reject a non-path self type. `decorator` names the caller for the
/// error message.
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

/// If `ty` syntactically matches `Arc<Inner>`, return `Inner`. Only the last
/// path segment is inspected (`std::sync::Arc<T>` works as well as `Arc<T>`).
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

/// A short, human-readable label for a dependency type, used in boot
/// diagnostics: the last path segment (`crate::a::Dep` → `Dep`), or `dyn Trait`
/// for a trait object. Falls back to the token rendering for anything exotic.
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

/// The `idx`-th generic type argument of `ty` when its last path segment is
/// `name<...>` — the building block for peeling a transport wrapper (`Json`,
/// `Result`, `Valid`, `Piped`) off a payload type. The wrapper-name-parameterised
/// generalisation of the `Arc`-specific `arc_inner`.
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
