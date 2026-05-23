//! Shared helpers for nestrs decorator macros.
//!
//! A procedural macro must live in a `proc-macro = true` crate, which can
//! export nothing but macros — so the token-building logic every decorator
//! shares (`#[injectable]`-style construction, the `from_container`
//! constructor, the `Discoverable::dependencies` list) lives here, in a plain
//! library crate that each `nestrs-*-macros` crate depends on. Third-party
//! decorator crates can depend on it too.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{ParseStream, Parser};
use syn::{
    Fields, FnArg, GenericArgument, Ident, ItemStruct, LitStr, Pat, PathArguments, Signature,
    Token, Type,
};

/// Parse a decorator's sole `<key> = "..."` string argument from its attribute
/// tokens — `#[controller(path = "…")]`, `#[cron_job(every = "…")]`, etc. `key`
/// is the expected argument name, `attr` the attribute; both appear in the error.
pub fn parse_named_str_arg(args: TokenStream2, key: &str, attr: &str) -> syn::Result<LitStr> {
    let parser = |input: ParseStream| -> syn::Result<LitStr> {
        let found: Ident = input.parse()?;
        if found != key {
            return Err(syn::Error::new(
                found.span(),
                format!("expected `{key} = \"...\"` as the only #[{attr}] argument"),
            ));
        }
        input.parse::<Token![=]>()?;
        input.parse()
    };
    parser.parse2(args)
}

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
fn arc_inner(ty: &Type) -> Option<&Type> {
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

/// If `ty` syntactically matches `Arc<dyn Trait + ...>`, return the inner
/// trait-object type so the macro can emit a `get_dyn::<dyn Trait + ...>()`
/// call.
fn arc_dyn_inner(ty: &Type) -> Option<&Type> {
    let inner = arc_inner(ty)?;
    matches!(inner, Type::TraitObject(_)).then_some(inner)
}

/// The constructor expression for a struct's `from_container`, plus the
/// `TypeId` expression of each `#[inject]` dependency. The dep keys feed the
/// `Discoverable::dependencies` impl that lets `#[module]` order registration.
pub struct InjectableBody {
    pub ctor: TokenStream2,
    pub dep_keys: Vec<TokenStream2>,
}

/// Strip `#[inject]` attributes from `item`'s fields and build its
/// `from_container` constructor expression, collecting each injected
/// dependency's `TypeId`. `#[inject] Arc<dyn Trait>` resolves via `get_dyn`;
/// `#[inject] Arc<Concrete>` via `get`; everything else falls back to
/// `Default::default()`.
pub fn build_injectable_body(item: &mut ItemStruct) -> syn::Result<InjectableBody> {
    match &mut item.fields {
        Fields::Unit => Ok(InjectableBody {
            ctor: quote! { Self },
            dep_keys: Vec::new(),
        }),
        Fields::Named(fields) => {
            let mut has_inject = false;
            let mut field_inits = Vec::new();
            let mut dep_keys = Vec::new();

            for field in fields.named.iter_mut() {
                let field_name = field.ident.clone().expect("named field has an ident");
                let inject_idx = field.attrs.iter().position(|a| a.path().is_ident("inject"));
                if let Some(idx) = inject_idx {
                    field.attrs.remove(idx);
                    has_inject = true;
                    let msg = format!(
                        "{}.{}: no provider registered for this dependency",
                        item.ident, field_name
                    );
                    let field_ty = &field.ty;
                    if let Some(trait_ty) = arc_dyn_inner(field_ty) {
                        field_inits.push(quote! {
                            #field_name: container.get_dyn::<#trait_ty>().expect(#msg)
                        });
                        // `provide_dyn` keys the binding by `TypeId::of::<Arc<dyn Trait>>()`,
                        // which is exactly this field's type.
                        dep_keys.push(quote! { ::core::any::TypeId::of::<#field_ty>() });
                    } else {
                        field_inits.push(quote! {
                            #field_name: container.get().expect(#msg)
                        });
                        // `get()` resolves the type inside `Arc<Inner>`, so that
                        // inner type is the dependency key.
                        let key_ty = match arc_inner(field_ty) {
                            Some(inner) => quote! { #inner },
                            None => quote! { #field_ty },
                        };
                        dep_keys.push(quote! { ::core::any::TypeId::of::<#key_ty>() });
                    }
                } else {
                    field_inits.push(quote! {
                        #field_name: ::core::default::Default::default()
                    });
                }
            }

            let ctor = if has_inject {
                quote! { Self { #(#field_inits),* } }
            } else {
                quote! { <Self as ::core::default::Default>::default() }
            };
            Ok(InjectableBody { ctor, dep_keys })
        }
        Fields::Unnamed(_) => Err(syn::Error::new_spanned(
            &item.fields,
            "#[injectable] does not support tuple structs",
        )),
    }
}

/// The `from_container` constructor every decorator macro emits, given a `ctor`
/// expression from [`build_injectable_body`].
pub fn from_container_method(ctor: &TokenStream2) -> TokenStream2 {
    quote! {
        pub fn from_container(container: &::nestrs_core::Container) -> Self {
            let _ = container;
            #ctor
        }
    }
}

/// The binding identifiers of a method's value arguments (the receiver is
/// skipped), in order, for forwarding a call to it. `#[routes]` and
/// `#[resolver]` both wrap a method in a generated one that re-invokes it by
/// name, so both need these.
///
/// Errors on any argument whose pattern is not a plain identifier (e.g. a
/// `Path(id)` destructure) — such a binding cannot be forwarded by name, and a
/// spanned error here beats the arity mismatch the generated call would
/// otherwise raise against macro-expanded code.
pub fn forwarded_arg_idents(sig: &Signature) -> syn::Result<Vec<Ident>> {
    forwarded_idents(&sig.inputs)
}

/// [`forwarded_arg_idents`] over an arbitrary argument sequence rather than a
/// whole signature. `#[resolver]`'s `#[field]` path drops the parent argument
/// before forwarding, so it passes the trimmed tail here.
pub fn forwarded_idents<'a>(
    inputs: impl IntoIterator<Item = &'a FnArg>,
) -> syn::Result<Vec<Ident>> {
    let mut idents = Vec::new();
    for arg in inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        match &*pat_type.pat {
            Pat::Ident(pat_ident) => idents.push(pat_ident.ident.clone()),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "resolver/controller method arguments must be simple identifiers \
                     (no destructuring patterns)",
                ))
            }
        }
    }
    Ok(idents)
}

/// The `Discoverable::dependencies` method for eagerly-built providers, listing
/// each `#[inject]` dependency's `TypeId` so `#[module]` can order registration.
pub fn dependencies_method(dep_keys: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn dependencies() -> ::std::vec::Vec<::core::any::TypeId> {
            ::std::vec![ #(#dep_keys),* ]
        }
    }
}
