//! `#[injectable]`-style construction: building a struct's `from_container`
//! constructor from its `#[inject]` fields, plus the `Discoverable` method bodies
//! every decorator emits.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Fields, FnArg, Ident, ItemStruct, Pat, Signature};

use crate::ty::{arc_inner, nth_generic_type, type_label};

/// The constructor expression for a struct's `from_container`, plus, per
/// `#[inject]` dependency (in field order), its `TypeId` expression and a
/// human-readable label. The keys feed `Discoverable::dependencies` (so
/// `#[module]` can order registration); the labels feed
/// `Discoverable::dependency_names` (so a boot-time error can name a missing one).
pub struct InjectableBody {
    pub ctor: TokenStream2,
    pub dep_keys: Vec<TokenStream2>,
    pub dep_names: Vec<TokenStream2>,
    /// `TypeId` of each `#[inject] Option<Arc<…>>` optional dependency. Kept apart
    /// from `dep_keys`: an optional dep must not gate the register-phase fixpoint
    /// the way a required one does (it tolerates absence), but the fixpoint still
    /// needs it to *order* the consumer after an optional provider that the same
    /// module does supply — see `Discoverable::optional_dependencies`.
    pub opt_keys: Vec<TokenStream2>,
}

/// Strip `#[inject]` attributes from `item`'s fields and build its
/// `from_container` constructor expression, collecting each injected
/// dependency's `TypeId` and label. `#[inject] Arc<dyn Trait>` resolves via
/// `get_dyn`, `#[inject] Arc<Concrete>` via `get`; an `#[inject] Option<Arc<…>>`
/// is an optional dependency (resolved leniently, excluded from
/// `dependencies`/`injected`); an `#[inject]` field that is neither is a hard
/// error (a dependency is always a shared `Arc`). A field without `#[inject]`
/// falls back to `Default::default()`.
pub fn build_injectable_body(item: &mut ItemStruct) -> syn::Result<InjectableBody> {
    match &mut item.fields {
        Fields::Unit => Ok(InjectableBody {
            ctor: quote! { Self },
            dep_keys: Vec::new(),
            dep_names: Vec::new(),
            opt_keys: Vec::new(),
        }),
        Fields::Named(fields) => {
            let mut has_inject = false;
            let mut field_inits = Vec::new();
            let mut dep_keys = Vec::new();
            let mut dep_names = Vec::new();
            let mut opt_keys = Vec::new();

            for field in fields.named.iter_mut() {
                let field_name = field.ident.clone().expect("named field has an ident");
                let inject_idx = field.attrs.iter().position(|a| a.path().is_ident("inject"));
                let Some(idx) = inject_idx else {
                    field_inits.push(quote! {
                        #field_name: ::core::default::Default::default()
                    });
                    continue;
                };
                field.attrs.remove(idx);
                has_inject = true;

                let field_ty = &field.ty;

                // Optional dependency: `#[inject] Option<Arc<T>>` /
                // `Option<Arc<dyn Trait>>` — the `@Optional` analog. Resolved
                // leniently (`None` when absent, no `.expect`) and excluded from
                // `dependencies`/`injected`, so it neither gates register ordering
                // nor fails the access-graph check when its provider is missing.
                if let Some(opt_inner) = nth_generic_type(field_ty, "Option", 0) {
                    let Some(arc_inner_ty) = arc_inner(opt_inner) else {
                        return Err(syn::Error::new_spanned(
                            field_ty,
                            "#[inject] `Option<…>` must wrap an `Arc<T>` or `Arc<dyn Trait>` \
                             (the optional-dependency form)",
                        ));
                    };
                    if matches!(arc_inner_ty, syn::Type::TraitObject(_)) {
                        field_inits.push(quote! {
                            #field_name: container.get_dyn::<#arc_inner_ty>()
                        });
                        // `provide_dyn` keys the binding by `Arc<dyn Trait>`,
                        // which is exactly `opt_inner`.
                        opt_keys.push(quote! { ::core::any::TypeId::of::<#opt_inner>() });
                    } else {
                        field_inits.push(quote! { #field_name: container.get() });
                        opt_keys.push(quote! { ::core::any::TypeId::of::<#arc_inner_ty>() });
                    }
                    continue;
                }

                let Some(inner_ty) = arc_inner(field_ty) else {
                    return Err(syn::Error::new_spanned(
                        field_ty,
                        "#[inject] requires an `Arc<T>` or `Arc<dyn Trait>` field — a \
                         dependency is resolved from the container as a shared `Arc`",
                    ));
                };
                let msg = format!(
                    "{}.{}: no provider registered for this dependency",
                    item.ident, field_name
                );
                let label = type_label(inner_ty);
                dep_names.push(quote! { #label });

                if matches!(inner_ty, syn::Type::TraitObject(_)) {
                    field_inits.push(quote! {
                        #field_name: container.get_dyn::<#inner_ty>().expect(#msg)
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
                    dep_keys.push(quote! { ::core::any::TypeId::of::<#inner_ty>() });
                }
            }

            let ctor = if has_inject {
                quote! { Self { #(#field_inits),* } }
            } else {
                quote! { <Self as ::core::default::Default>::default() }
            };
            Ok(InjectableBody {
                ctor,
                dep_keys,
                dep_names,
                opt_keys,
            })
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

/// A `::std::vec![...]` of a provider's `#[inject]` dependency `TypeId`s — the
/// shared body behind [`dependencies_method`] / [`injected_method`], and the form
/// a decorator emits outside a trait method (`#[controller]` emits it as an
/// inherent fn that `#[routes]` reads back into `Discoverable::injected`).
pub fn injected_keys_expr(dep_keys: &[TokenStream2]) -> TokenStream2 {
    quote! { ::std::vec![ #(#dep_keys),* ] }
}

/// The `Discoverable::dependencies` method for eagerly-built providers, listing
/// each `#[inject]` dependency's `TypeId` so `#[module]` can order registration.
pub fn dependencies_method(dep_keys: &[TokenStream2]) -> TokenStream2 {
    let body = injected_keys_expr(dep_keys);
    quote! {
        fn dependencies() -> ::std::vec::Vec<::core::any::TypeId> {
            #body
        }
    }
}

/// The `Discoverable::dependency_names` method — a human-readable label per
/// `#[inject]` dependency, index-aligned with [`dependencies_method`], so the
/// `#[module]` boot-time fixpoint can name a missing dependency in its error.
/// Emitted only by eager providers (those that also emit `dependencies`), since
/// only they can stall the fixpoint.
pub fn dependency_names_method(dep_names: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn dependency_names() -> ::std::vec::Vec<&'static str> {
            ::std::vec![ #(#dep_names),* ]
        }
    }
}

/// The `Discoverable::optional_dependencies` method, listing each
/// `#[inject] Option<Arc<…>>` key. The `#[module]` fixpoint uses it to order an
/// eager provider after an optional dependency the same module *does* supply,
/// while still building it (with `None`) when no provider supplies one.
pub fn optional_dependencies_method(opt_keys: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn optional_dependencies() -> ::std::vec::Vec<::core::any::TypeId> {
            ::std::vec![ #(#opt_keys),* ]
        }
    }
}

/// The `Discoverable::injected` method: the same `#[inject]` dependency keys as
/// [`dependencies_method`], reported for the module access-graph check. Kept
/// distinct from `dependencies` because a provider built later
/// from the fully-assembled container (a controller, cron job, or processor)
/// must report what it injects *without* forcing those dependencies to precede
/// its own registration — its `dependencies` stays empty, its `injected` does not.
pub fn injected_method(dep_keys: &[TokenStream2]) -> TokenStream2 {
    let body = injected_keys_expr(dep_keys);
    quote! {
        fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
            #body
        }
    }
}
