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

/// The last path segment's ident and its angle-bracketed type arguments, when
/// `ty` is a path type with generics: `a::b::Piped<P, T>` ⇒ `("Piped", [P, T])`.
///
/// The primitive under [`nth_generic_type`] and [`pipe_wrapper`] — matching on
/// the *last* segment is what makes a fully-qualified spelling work everywhere
/// a bare one does.
pub fn generic_args(ty: &Type) -> Option<(&Ident, Vec<&Type>)> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let tys = args
        .args
        .iter()
        .filter_map(|arg| match arg {
            GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .collect();
    Some((&seg.ident, tys))
}

/// A per-argument pipe binding read off a parameter's type — the shape
/// `#[resolver]`, `#[messages]` and `#[processor]` each need to strip before
/// deserializing the wire value.
pub enum PipeWrapper {
    /// `Piped<P, T>` — run pipe `P` over the wire value `T`.
    Piped {
        /// The pipe type to apply.
        pipe: syn::Path,
        /// The value that crosses the wire, and what the pipe consumes.
        value: Type,
    },
    /// `Valid<T>` — validate the wire value `T`.
    Valid {
        /// The value that crosses the wire.
        value: Type,
    },
}

impl PipeWrapper {
    /// The wire value the transport deserializes, whichever wrapper this is.
    pub fn value(&self) -> &Type {
        match self {
            Self::Piped { value, .. } | Self::Valid { value } => value,
        }
    }
}

/// Recognise `Piped<P, T>` / `Valid<T>` on `ty`'s last path segment.
///
/// The three non-HTTP transports bind pipes per argument with exactly this
/// pair of wrappers (HTTP wraps an extractor instead — orphan rule), and each
/// macro used to re-derive the match from scratch. `None` ⇒ a plain payload,
/// deserialized as-is.
pub fn pipe_wrapper(ty: &Type) -> Option<PipeWrapper> {
    let (ident, tys) = generic_args(ty)?;
    match (ident.to_string().as_str(), tys.as_slice()) {
        ("Piped", [Type::Path(pipe), value]) => Some(PipeWrapper::Piped {
            pipe: pipe.path.clone(),
            value: (*value).clone(),
        }),
        ("Valid", [value]) => Some(PipeWrapper::Valid {
            value: (*value).clone(),
        }),
        _ => None,
    }
}

/// The last segment's ident of a path — the readable name in a diagnostic or a
/// generated identifier. `syn::Path` always has at least one segment.
pub fn last_segment_ident(path: &syn::Path) -> &Ident {
    &path
        .segments
        .last()
        .expect("syn::Path has ≥ 1 segment")
        .ident
}

/// Short label for a dependency type in diagnostics: last path segment, or
/// `dyn Trait` for a trait object, or the token rendering otherwise.
pub fn type_label(ty: &Type) -> String {
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

/// The single payload argument of an orchestrator method: `&self` receiver,
/// then exactly one typed parameter.
///
/// `#[on_event]` and `#[process]` impose the identical signature and produced
/// identical diagnostics up to one noun; `decorator` (`"#[process]"`) and
/// `payload` (`"job"`) are what actually differed. Extra dependencies belong on
/// the host struct as `#[inject]` fields, which is why more than one argument
/// is refused rather than resolved.
pub fn payload_arg_type(
    method: &syn::ImplItemFn,
    decorator: &str,
    payload: &str,
) -> syn::Result<Type> {
    use syn::spanned::Spanned;
    use syn::{FnArg, PatType};

    let mut iter = method.sig.inputs.iter();
    match iter.next() {
        Some(FnArg::Receiver(_)) => {}
        Some(other) => {
            return Err(syn::Error::new(
                other.span(),
                format!("a `{decorator}` method must take `&self` as its first argument"),
            ));
        }
        None => {
            return Err(syn::Error::new(
                method.sig.span(),
                format!("a `{decorator}` method must take `&self` and one {payload} argument"),
            ));
        }
    }
    let Some(arg) = iter.next() else {
        return Err(syn::Error::new(
            method.sig.span(),
            format!(
                "a `{decorator}` method needs a {payload} argument: \
                 `async fn(&self, {payload}: T)`"
            ),
        ));
    };
    if iter.next().is_some() {
        return Err(syn::Error::new(
            method.sig.span(),
            format!(
                "a `{decorator}` method takes exactly one {payload} argument — extra \
                 dependencies belong on the host struct as `#[inject]` fields"
            ),
        ));
    }
    match arg {
        FnArg::Typed(PatType { ty, .. }) => Ok((**ty).clone()),
        FnArg::Receiver(r) => Err(syn::Error::new(
            r.span(),
            format!("a `{decorator}` method takes exactly one `&self` receiver"),
        )),
    }
}

/// The shared **message** for the edge rule "a resource id is a UUID v7".
///
/// The rule is one edge decision, but each transport rejects in its own error
/// type — so what is genuinely shared is the wording, and that is what had
/// drifted (`"path id must be a UUID v7"` on HTTP against `"id must be a UUID
/// v7"` on GraphQL, for the same rejection). Each `#[crud]` builds its own
/// `return Err(...)` around this constant.
pub const UUID_V7_REQUIRED: &str = "id must be a UUID v7";
