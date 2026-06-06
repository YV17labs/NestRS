//! `#[injectable]`-style construction: build a struct's `from_container`
//! constructor from its `#[inject]` fields plus the `Discoverable` method
//! bodies every decorator emits.

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::{Fields, FnArg, Ident, ItemStruct, Pat, Path, Signature};

use crate::ty::{arc_inner, nth_generic_type, type_label};

/// The constructor expression plus, per `#[inject]` dependency, its `TypeId`
/// expression and a human-readable label.
pub struct InjectableBody {
    pub ctor: TokenStream2,
    pub dep_keys: Vec<TokenStream2>,
    pub dep_names: Vec<TokenStream2>,
    /// `TypeId` of each `#[inject] Option<Arc<…>>`. Kept apart from `dep_keys`
    /// — optionals must not gate the register fixpoint, but are still used to
    /// order a consumer after an optional provider the same module supplies.
    pub opt_keys: Vec<TokenStream2>,
}

/// Strip `#[inject]` attributes from `item`'s fields and build its
/// `from_container` constructor. `Arc<dyn Trait>` resolves via `get_dyn`,
/// `Arc<Concrete>` via `get`. `Option<Arc<…>>` is an optional dependency
/// (lenient, excluded from `dependencies`/`injected`). An `#[inject]` field
/// that is neither errors; a non-`#[inject]` field falls back to
/// `Default::default()`.
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

                // Optional `#[inject] Option<Arc<…>>`: lenient resolution,
                // out of `dependencies`/`injected` so a missing provider
                // neither stalls the register fixpoint nor fails access check.
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
                        // `provide_dyn` keys by `Arc<dyn Trait>` = `opt_inner`.
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
                    // `provide_dyn` keys by `Arc<dyn Trait>` = `field_ty`.
                    dep_keys.push(quote! { ::core::any::TypeId::of::<#field_ty>() });
                } else {
                    field_inits.push(quote! {
                        #field_name: container.get().expect(#msg)
                    });
                    // `get()` keys by the type inside `Arc<…>`.
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

/// The `from_container` constructor emitted by every decorator macro.
pub fn from_container_method(ctor: &TokenStream2) -> TokenStream2 {
    quote! {
        pub fn from_container(container: &::nest_rs_core::Container) -> Self {
            let _ = container;
            #ctor
        }
    }
}

/// Binding identifiers of a method's value arguments (receiver skipped) for
/// forwarding a call by name. Errors on a non-identifier pattern (e.g.
/// `Path(id)` destructure) — a spanned error beats the arity mismatch the
/// generated call would otherwise raise.
pub fn forwarded_arg_idents(sig: &Signature) -> syn::Result<Vec<Ident>> {
    forwarded_idents(&sig.inputs)
}

/// [`forwarded_arg_idents`] over an arbitrary argument sequence — used when
/// `#[resolver]`'s `#[field_resolver]` path drops the parent before forwarding.
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
                ));
            }
        }
    }
    Ok(idents)
}

/// `TypeId::of::<P>()` for each referenced type a provider resolves from the
/// container outside its `#[inject]` fields — guards, filters, interceptors,
/// resolver `#[field_resolver]` `&Service` deps — deduplicated by token text. Feeding
/// these into `Discoverable::injected` puts them under the access contract.
pub fn layer_inject_keys<'a, T: ToTokens + 'a>(
    items: impl IntoIterator<Item = &'a T>,
) -> Vec<TokenStream2> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|p| seen.insert(quote!(#p).to_string()))
        .map(|p| quote! { ::core::any::TypeId::of::<#p>() })
        .collect()
}

/// `::std::vec![...]` of `#[inject]` dependency `TypeId`s — body for
/// [`dependencies_method`]/[`injected_method`] and for the inherent
/// `__nestrs_injected()` a struct decorator emits.
pub fn injected_keys_expr(dep_keys: &[TokenStream2]) -> TokenStream2 {
    quote! { ::std::vec![ #(#dep_keys),* ] }
}

/// `injected_keys_expr` extended with the dedup'd struct-level guard/filter/
/// interceptor `TypeId`s; the companion impl-block macro appends per-route/
/// per-message layers on top via [`injected_method_with_layers`].
pub fn injected_keys_with_layers<'a>(
    dep_keys: &[TokenStream2],
    layer_paths: impl IntoIterator<Item = &'a Path>,
) -> TokenStream2 {
    let mut keys = dep_keys.to_vec();
    keys.extend(layer_inject_keys(layer_paths));
    injected_keys_expr(&keys)
}

/// `Discoverable::injected` for an impl-block macro: take the struct's
/// `__nestrs_injected()` and extend it with per-route/per-message layer
/// `TypeId`s. The fixed-size, explicitly-typed array keeps `extend` unambiguous
/// when no per-method layers are present.
pub fn injected_method_with_layers(
    self_ty: &impl quote::ToTokens,
    layer_keys: &[TokenStream2],
) -> TokenStream2 {
    let count = proc_macro2::Literal::usize_unsuffixed(layer_keys.len());
    quote! {
        fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
            let mut __keys = <#self_ty>::__nestrs_injected();
            let __layers: [::core::any::TypeId; #count] = [ #(#layer_keys),* ];
            __keys.extend(__layers);
            __keys
        }
    }
}

/// `Discoverable::dependencies` for eagerly-built providers — drives
/// register-phase ordering.
pub fn dependencies_method(dep_keys: &[TokenStream2]) -> TokenStream2 {
    let body = injected_keys_expr(dep_keys);
    quote! {
        fn dependencies() -> ::std::vec::Vec<::core::any::TypeId> {
            #body
        }
    }
}

/// `Discoverable::dependency_names` — index-aligned with
/// [`dependencies_method`]; only eager providers emit it (only they can stall
/// the fixpoint).
pub fn dependency_names_method(dep_names: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn dependency_names() -> ::std::vec::Vec<&'static str> {
            ::std::vec![ #(#dep_names),* ]
        }
    }
}

/// `Discoverable::optional_dependencies` — orders an eager provider after an
/// optional dep the same module supplies, while still building it (with
/// `None`) when no provider supplies one.
pub fn optional_dependencies_method(opt_keys: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn optional_dependencies() -> ::std::vec::Vec<::core::any::TypeId> {
            ::std::vec![ #(#opt_keys),* ]
        }
    }
}

/// `Discoverable::injected` for the access-graph check. Distinct from
/// `dependencies`: a lazily-built provider (controller, cron job, processor)
/// reports what it injects without forcing those deps to precede its own
/// registration.
pub fn injected_method(dep_keys: &[TokenStream2]) -> TokenStream2 {
    let body = injected_keys_expr(dep_keys);
    quote! {
        fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
            #body
        }
    }
}
