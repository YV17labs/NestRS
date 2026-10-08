//! Type/path inspection helpers shared by the decorator macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{GenericArgument, Ident, PathArguments, Type, TypeParamBound};

use crate::ungrouped::ungrouped_type;

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
/// Matching on the *last* segment, so a fully-qualified spelling works too.
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
/// The three non-HTTP transports bind pipes per argument with this pair (HTTP
/// wraps an extractor instead — orphan rule). `None` ⇒ a plain payload.
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
#[expect(
    clippy::expect_used,
    reason = "syn::Path always has at least one segment"
)]
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

/// `call`, awaited when `sig` is an `async fn` and left as it is otherwise.
pub fn await_if_async(sig: &syn::Signature, call: TokenStream) -> TokenStream {
    if sig.asyncness.is_some() {
        quote!(#call.await)
    } else {
        call
    }
}

/// Whether a method answers `()` — the return written `-> ()`, or not written at
/// all.
///
/// A `()` wrapped in a `macro_rules!` invisible group is the same `()`.
pub fn returns_unit(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => true,
        syn::ReturnType::Type(_, ty) => {
            matches!(ungrouped_type(ty), Type::Tuple(tuple) if tuple.elems.is_empty())
        }
    }
}

/// What an impl half's expansion calls a decorated method through — the one
/// thing about the receiver that differs between the nine decorators.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HostBorrow {
    /// A borrow of the host alone: an edge dispatches from code holding `&Self`.
    Host,
    /// A borrow of the `Arc` the container holds a provider in, so
    /// `self: &Arc<Self>` is lent as well as `&self`.
    Arc,
}

/// Refuse any receiver the expansion's call cannot lend — anything but `&self`,
/// a typed shared borrow of the host, or (where `borrow` is [`HostBorrow::Arc`])
/// of the `Arc` holding it — with a sentence quoting the one written.
///
/// A host is one instance shared by every call, so there is no exclusive
/// access or ownership to take. `decorator` is the attribute the reader wrote;
/// `host` is the impl's type name, which a pointer's argument is read against.
pub fn shared_receiver(
    method: &syn::ImplItemFn,
    decorator: &str,
    host: &Ident,
    borrow: HostBorrow,
) -> syn::Result<()> {
    let refusal = |at: &dyn quote::ToTokens, written: &str| {
        let accepted = match borrow {
            HostBorrow::Host => "`&self`",
            HostBorrow::Arc => "`&self`, or `self: &Arc<Self>`",
        };
        syn::Error::new_spanned(
            at,
            format!(
                "a `{decorator}` method borrows its host — {accepted} — and this one takes \
                 {written}: a host is one instance shared by every call, and each method is \
                 lent a shared borrow of it"
            ),
        )
    };
    match method.sig.inputs.first() {
        Some(syn::FnArg::Receiver(receiver)) if borrows_shared(receiver, host, borrow) => Ok(()),
        Some(syn::FnArg::Receiver(receiver)) => Err(refusal(
            receiver,
            &format!("`{}`", receiver_as_written(receiver)),
        )),
        Some(first) => Err(refusal(first, "no receiver")),
        None => Err(refusal(&method.sig, "no receiver")),
    }
}

/// Refuse a type or const parameter on a dispatched method, naming the first.
///
/// Nothing at the call names the type, so rustc would answer `E0283` at the
/// decorator. A lifetime is not refused. `decorator` is the attribute the
/// reader wrote.
pub fn concrete_signature(method: &syn::ImplItemFn, decorator: &str) -> syn::Result<()> {
    let written = method
        .sig
        .generics
        .params
        .iter()
        .find_map(|param| match param {
            syn::GenericParam::Lifetime(_) => None,
            syn::GenericParam::Type(ty) => Some((param, "type", "a type", &ty.ident)),
            syn::GenericParam::Const(konst) => Some((param, "const", "a value", &konst.ident)),
        });
    match written {
        None => Ok(()),
        Some((param, kind, supplied, name)) => Err(syn::Error::new_spanned(
            param,
            format!(
                "a `{decorator}` method takes no type or const parameters, and `{method}` \
                 declares {kind} parameter `{name}`: the expansion calls it with only what its \
                 transport carries, so nothing supplies {supplied} for it — name a concrete \
                 one, or call a generic function from the body",
                method = method.sig.ident,
            ),
        )),
    }
}

/// `&self` or `&'a self`, or a typed shared borrow of the host (`&Self`, or any
/// type rustc then judges) or, where `borrow` lends one, of its `Arc`. Another
/// pointer around the host, or a `'static` borrow, is refused; the reading is
/// textual, so an aliased pointer is read by its alias.
fn borrows_shared(receiver: &syn::Receiver, host: &Ident, borrow: HostBorrow) -> bool {
    match &receiver.kind {
        syn::ReceiverKind::Reference(_, lifetime, mutability) => {
            mutability.is_none() && !is_static(lifetime.as_ref())
        }
        syn::ReceiverKind::Typed(_, ty) => match ungrouped_type(ty) {
            Type::Reference(reference)
                if reference.mutability.is_none() && !is_static(reference.lifetime.as_ref()) =>
            {
                match ungrouped_type(&reference.elem) {
                    Type::Path(path) if path.qself.is_none() => {
                        path.path.segments.last().is_some_and(|segment| {
                            (borrow == HostBorrow::Arc && is_arc_of_host(segment, host))
                                || !wraps_host(segment, host)
                        })
                    }
                    _ => false,
                }
            }
            _ => false,
        },
        _ => false,
    }
}

fn is_static(lifetime: Option<&syn::Lifetime>) -> bool {
    lifetime.is_some_and(|lifetime| lifetime.ident == "static")
}

/// Whether `segment` is `Arc<Self>` or `Arc<Host..>`: the `Arc` the container
/// holds the host in, its one type argument the host itself.
fn is_arc_of_host(segment: &syn::PathSegment, host: &Ident) -> bool {
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    let mut types = arguments.args.iter().filter_map(|argument| match argument {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    segment.ident == "Arc"
        && matches!((types.next(), types.next()), (Some(ty), None) if is_host(ty, host))
}

/// Whether `segment` is a pointer around the host: a type argument of it names the
/// host, behind any references or pointers.
fn wraps_host(segment: &syn::PathSegment, host: &Ident) -> bool {
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    arguments.args.iter().any(|argument| match argument {
        syn::GenericArgument::Type(ty) => names_host(ty, host),
        _ => false,
    })
}

/// Whether `ty` names the host anywhere: itself, behind references, or as a type
/// argument of a path.
fn names_host(ty: &Type, host: &Ident) -> bool {
    match ungrouped_type(ty) {
        Type::Reference(reference) => names_host(&reference.elem, host),
        Type::Path(path) if path.qself.is_none() => {
            is_host(ty, host)
                || path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| wraps_host(segment, host))
        }
        _ => false,
    }
}

/// Whether `ty` is the host: `Self`, or a path ending in its name, raw or not.
fn is_host(ty: &Type, host: &Ident) -> bool {
    use syn::ext::IdentExt;

    matches!(
        ungrouped_type(ty),
        Type::Path(path)
            if path.qself.is_none()
                && path.path.segments.last().is_some_and(|segment| {
                    segment.ident == "Self" || segment.ident.unraw() == host.unraw()
                })
    )
}

/// The receiver as its author wrote it, for the refusal to quote.
fn receiver_as_written(receiver: &syn::Receiver) -> String {
    let binding = if receiver.mutability.is_some() {
        "mut "
    } else {
        ""
    };
    match &receiver.kind {
        syn::ReceiverKind::Reference(_, lifetime, mutability) => format!(
            "&{}{}self",
            lifetime
                .as_ref()
                .map(|lifetime| format!("{lifetime} "))
                .unwrap_or_default(),
            if mutability.is_some() { "mut " } else { "" },
        ),
        syn::ReceiverKind::Typed(_, ty) => format!(
            "{binding}self: {}",
            quote!(#ty)
                .to_string()
                .replace(" < ", "<")
                .replace("< ", "<")
                .replace(" >", ">")
                .replace(" :: ", "::")
                .replace("& ", "&")
        ),
        _ => format!("{binding}self"),
    }
}

/// The single payload argument of an orchestrator method: `&self` receiver,
/// then exactly one typed parameter.
///
/// `decorator` (`"#[process]"`) and `payload` (`"job"`) word the refusals;
/// extra dependencies belong on the host as `#[inject]` fields. `host` is the
/// impl's type name, which a typed receiver may borrow through its `Arc`.
pub fn payload_arg_type(
    method: &syn::ImplItemFn,
    decorator: &str,
    payload: &str,
    host: &Ident,
) -> syn::Result<Type> {
    use syn::spanned::Spanned;
    use syn::{FnArg, PatType};

    shared_receiver(method, decorator, host, HostBorrow::Arc)?;
    let mut iter = method.sig.inputs.iter().skip(1);
    let Some(arg) = iter.next() else {
        return Err(syn::Error::new(
            method.sig.span(),
            format!(
                "a `{decorator}` method needs a {payload} argument: \
                 `fn(&self, {payload}: T)`"
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

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn receiver_answer(method: &syn::ImplItemFn, host: Ident) -> Result<(), String> {
        shared_receiver(method, "#[hooks]", &host, HostBorrow::Arc)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn a_borrow_the_call_can_lend_is_accepted_however_it_is_spelled() {
        let methods: [syn::ImplItemFn; 9] = [
            parse_quote! { async fn plain(&self) {} },
            parse_quote! { async fn lifetime<'a>(&'a self) {} },
            parse_quote! { async fn typed(self: &Self) {} },
            parse_quote! { async fn named(self: &Host) {} },
            parse_quote! { async fn generic(self: &Host<u8>) {} },
            parse_quote! { async fn pathed(self: &crate::Host) {} },
            parse_quote! { async fn aliased(self: &HostAlias) {} },
            parse_quote! { async fn arc(self: &Arc<Self>) {} },
            parse_quote! { async fn std_arc(self: &std::sync::Arc<Host<u8>>) {} },
        ];
        for method in &methods {
            assert_eq!(
                receiver_answer(method, parse_quote!(Host)),
                Ok(()),
                "{}",
                method.sig.ident
            );
        }
        let raw: syn::ImplItemFn = parse_quote! { async fn raw(self: &Arc<Raw>) {} };
        assert_eq!(receiver_answer(&raw, parse_quote!(r#Raw)), Ok(()));
    }

    #[test]
    fn any_other_receiver_is_refused_quoting_it() {
        let cases: Vec<(syn::ImplItemFn, &str)> = vec![
            (
                parse_quote! { async fn exclusive(&mut self) {} },
                "`&mut self`",
            ),
            (parse_quote! { async fn owned(self) {} }, "`self`"),
            (
                parse_quote! { async fn arc(self: std::sync::Arc<Self>) {} },
                "`self: std::sync::Arc<Self>`",
            ),
            (
                parse_quote! { async fn boxed(self: &Box<Self>) {} },
                "`self: &Box<Self>`",
            ),
            (
                parse_quote! { async fn counted(self: &std::rc::Rc<Host>) {} },
                "`self: &std::rc::Rc<Host>`",
            ),
            (
                parse_quote! { async fn pinned(self: &Pin<&Self>) {} },
                "`self: &Pin<&Self>`",
            ),
            (
                parse_quote! { async fn pinned_box(self: &Pin<Box<Self>>) {} },
                "`self: &Pin<Box<Self>>`",
            ),
            (
                parse_quote! { async fn arc_of_box(self: &Arc<Box<Self>>) {} },
                "`self: &Arc<Box<Self>>`",
            ),
            (
                parse_quote! { async fn forever(&'static self) {} },
                "`&'static self`",
            ),
            (
                parse_quote! { async fn typed_forever(self: &'static Self) {} },
                "`self: &'static Self`",
            ),
            (parse_quote! { async fn none() {} }, "no receiver"),
        ];
        for (method, written) in &cases {
            let refusal = receiver_answer(method, parse_quote!(Host)).expect_err(written);
            assert!(refusal.contains(written), "{refusal}");
            assert!(
                refusal.contains("one instance shared by every call"),
                "{refusal}"
            );
        }
        let raw: syn::ImplItemFn = parse_quote! { async fn raw(self: &Box<Raw>) {} };
        assert!(
            receiver_answer(&raw, parse_quote!(r#Raw)).is_err(),
            "a raw host is its name"
        );
    }

    #[test]
    fn an_edge_refuses_the_arc_borrow_a_provider_takes() {
        let arc: syn::ImplItemFn = parse_quote! { async fn arc(self: &Arc<Self>) {} };
        let refusal = shared_receiver(&arc, "#[routes]", &parse_quote!(Host), HostBorrow::Host)
            .expect_err("an edge holds no Arc to lend")
            .to_string();
        assert!(
            refusal.contains("borrows its host — `&self` — and"),
            "{refusal}"
        );
        let plain: syn::ImplItemFn = parse_quote! { fn plain(&self) {} };
        assert!(
            shared_receiver(&plain, "#[routes]", &parse_quote!(Host), HostBorrow::Host).is_ok()
        );
    }
}
