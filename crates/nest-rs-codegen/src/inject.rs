//! `#[injectable]`-style construction: build a struct's `from_container`
//! constructor from its `#[inject]` fields plus the `Discoverable` method
//! bodies every decorator emits.

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::{Fields, FnArg, Ident, ItemStruct, Pat, Signature};

use crate::ty::{arc_inner, nth_generic_type, type_label};

/// The constructor expression plus, per `#[inject]` dependency, its `TypeId`
/// expression and a human-readable label.
pub struct InjectableBody {
    /// The struct-literal constructor expression that builds `Self` from the
    /// resolved `#[inject]` fields.
    pub ctor: TokenStream2,
    /// `TypeId` expression for each required `#[inject]` dependency.
    pub dep_keys: Vec<TokenStream2>,
    /// Human-readable label for each entry in `dep_keys`, in the same order.
    pub dep_names: Vec<TokenStream2>,
    /// `TypeId` of each `#[inject] Option<Arc<…>>`. Kept apart from `dep_keys`
    /// — optionals must not gate the register fixpoint, but are still used to
    /// order a consumer after an optional provider the same module supplies.
    pub opt_keys: Vec<TokenStream2>,
    /// One `::nest_rs_core::KeyedDependency { … }` expression per
    /// `#[inject(key = "…")]` field, for the access-graph keyed check. Kept
    /// apart from `dep_keys`: a keyed dependency resolves via `get_keyed`, is
    /// excluded from the register-phase fixpoint, and is validated against the
    /// global keyed set rather than the module import closure.
    pub keyed_dep_keys: Vec<TokenStream2>,
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
            keyed_dep_keys: Vec::new(),
        }),
        Fields::Named(fields) => {
            let mut has_inject = false;
            let mut field_inits = Vec::new();
            let mut dep_keys = Vec::new();
            let mut dep_names = Vec::new();
            let mut opt_keys = Vec::new();
            let mut keyed_dep_keys = Vec::new();

            for field in fields.named.iter_mut() {
                let field_name = field.ident.clone().expect("named field has an ident");
                let inject_idx = field.attrs.iter().position(|a| a.path().is_ident("inject"));
                let Some(idx) = inject_idx else {
                    // CORE-I5: an `Arc<…>` (or `Option<Arc<…>>`) field with no
                    // `#[inject]` is almost always a *forgotten* injection.
                    // Silently `Default::default()`-ing it — an empty config, a
                    // no-op guard/strategy — is a security footgun, so reject it.
                    let is_arc = arc_inner(&field.ty).is_some();
                    let is_opt_arc = nth_generic_type(&field.ty, "Option", 0)
                        .is_some_and(|inner| arc_inner(inner).is_some());
                    if is_arc || is_opt_arc {
                        return Err(syn::Error::new_spanned(
                            &field.ty,
                            "an `Arc<…>` field without `#[inject]` would be silently \
                             `Default::default()`-d (an empty config, a no-op guard/strategy) — \
                             add `#[inject]` to resolve it from the container, or store the \
                             default behind a non-`Arc` type if that is truly intended",
                        ));
                    }
                    field_inits.push(quote! {
                        #field_name: ::core::default::Default::default()
                    });
                    continue;
                };
                let inject_attr = field.attrs.remove(idx);
                has_inject = true;

                let field_ty = &field.ty;

                // A keyed `#[inject(key = "…")]` field resolves a keyed
                // singleton via `get_keyed`. Singleton-only, concrete `Arc<T>`
                // only — a key on an `Option<…>` or `Arc<dyn Trait>` field is a
                // compile error (no keyed optional/dyn resolution exists).
                if let Some(key) = parse_inject_key(&inject_attr)? {
                    if nth_generic_type(field_ty, "Option", 0).is_some() {
                        return Err(syn::Error::new_spanned(
                            field_ty,
                            "#[inject(key = \"…\")] does not support `Option<…>` — a keyed \
                             dependency is a required singleton",
                        ));
                    }
                    let Some(inner_ty) = arc_inner(field_ty) else {
                        return Err(syn::Error::new_spanned(
                            field_ty,
                            "#[inject(key = \"…\")] requires an `Arc<T>` field",
                        ));
                    };
                    if matches!(inner_ty, syn::Type::TraitObject(_)) {
                        return Err(syn::Error::new_spanned(
                            field_ty,
                            "#[inject(key = \"…\")] does not support `Arc<dyn Trait>` — keyed \
                             providers are concrete singletons",
                        ));
                    }
                    let msg = format!(
                        "{}.{}: no keyed provider registered for key `{}`",
                        item.ident,
                        field_name,
                        key.value()
                    );
                    field_inits.push(quote! {
                        #field_name: container.get_keyed(#key).expect(#msg)
                    });
                    let label = type_label(inner_ty);
                    keyed_dep_keys.push(quote! {
                        ::nest_rs_core::KeyedDependency {
                            key: ::nest_rs_core::ProviderKey::named::<#inner_ty>(#key),
                            type_name: #label,
                        }
                    });
                    continue;
                }

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
                keyed_dep_keys,
            })
        }
        Fields::Unnamed(_) => Err(syn::Error::new_spanned(
            &item.fields,
            "#[injectable] does not support tuple structs",
        )),
    }
}

/// Parse the optional `key = "…"` argument of an `#[inject]` attribute.
/// `#[inject]` (bare) yields `None`; `#[inject(key = "github")]` yields the
/// literal. Any other argument is a spanned compile error.
fn parse_inject_key(attr: &syn::Attribute) -> syn::Result<Option<syn::LitStr>> {
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return Ok(None);
    }
    let mut key: Option<syn::LitStr> = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("key") {
            crate::once(key.is_some(), &meta.path, "inject", "key")?;
            // `meta.value()` on a bare `#[inject(key)]` is syn's `` expected `=` ``,
            // which names the grammar and not the key — the silence
            // `needs_a_value` exists to end.
            if !meta.input.peek(syn::Token![=]) {
                return Err(meta.error(crate::needs_a_value("inject", "key")));
            }
            key = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error(crate::unknown_argument(
                "inject",
                &crate::key_as_written(&meta.path),
                &["key"],
            )))
        }
    })?;
    Ok(key)
}

/// `Discoverable::injected_keyed` — one `KeyedDependency` per
/// `#[inject(key = "…")]` field, for the access-graph keyed check.
pub fn injected_keyed_method(keyed_dep_keys: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn injected_keyed() -> ::std::vec::Vec<::nest_rs_core::KeyedDependency> {
            ::std::vec![ #(#keyed_dep_keys),* ]
        }
    }
}

/// The `from_container` constructor emitted by every decorator macro.
pub fn from_container_method(ctor: &TokenStream2) -> TokenStream2 {
    quote! {
        /// Construct this provider by resolving its `#[inject]` fields from the
        /// container. Emitted by the decorator; called by the register phase,
        /// not by hand.
        pub fn from_container(container: &::nest_rs_core::Container) -> Self {
            let _ = container;
            #ctor
        }
    }
}

/// The scope-aware constructor emitted by `#[injectable(scope = request)]`.
/// Identical body to [`from_container_method`], but the parameter is a
/// `&RequestScope` — so a `#[inject]` dep that is itself request-scoped
/// resolves through the per-request cache (and is shared with the rest of the
/// request), while singleton / keyed / `dyn` deps forward to the root. The
/// binding is named `container` so the shared `ctor` tokens
/// (`container.get()`, `container.get_dyn()`, `container.get_keyed()`) compile
/// unchanged against `RequestScope`'s matching
/// resolution methods.
pub fn from_scope_method(ctor: &TokenStream2) -> TokenStream2 {
    quote! {
        /// Construct this request-scoped provider from the per-request scope,
        /// so request-scoped `#[inject]` deps share the request's instances.
        /// Emitted by `#[injectable(scope = request)]`; called per request.
        pub fn from_scope(container: &::nest_rs_core::RequestScope) -> Self {
            let _ = container;
            #ctor
        }
    }
}

/// Binding identifiers of a method's value arguments (receiver skipped) for
/// forwarding a call by name. A destructuring pattern forwards under the
/// identifier it binds — see [`forwarded_idents`].
pub fn forwarded_arg_idents(sig: &Signature) -> syn::Result<Vec<Ident>> {
    forwarded_idents(&sig.inputs)
}

/// [`forwarded_arg_idents`] over an arbitrary argument sequence — used when
/// `#[resolver]`'s `#[field_resolver]` path drops the parent before forwarding.
///
/// A **destructuring pattern** resolves to the identifier it binds:
/// `Path(name): Path<String>` forwards as `name`, `Valid(Json(input))` as
/// `input`. That is poem's own idiom and the first shape a reader writes, so the
/// decorators accept it rather than making the developer un-destructure for the
/// macro's benefit. The developer's method keeps the pattern (it is valid Rust
/// there, and the macro re-emits the method unchanged); only the generated
/// wrapper's parameter list is rewritten, by
/// [`normalize_forwarded_args`] — a wrapper that destructured too would hand the
/// inner value (`String`) to a method expecting the extractor (`Path<String>`).
///
/// A pattern binding **no** name or **several** is still an error: there is no
/// single name to forward under, and on GraphQL the wrapper's parameter name is
/// the SDL argument name, so a synthesized one would leak into the schema.
pub fn forwarded_idents<'a>(
    inputs: impl IntoIterator<Item = &'a FnArg>,
) -> syn::Result<Vec<Ident>> {
    let mut idents = Vec::new();
    for arg in inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        idents.push(binder_of(&pat_type.pat)?);
    }
    Ok(idents)
}

/// An identifier for a local **the expansion binds for itself**, on
/// `Span::mixed_site()` — the span that resolves local variables at the macro's
/// *definition* site instead of the call site.
///
/// Reach for this for every `let` a wrapper introduces alongside identifiers the
/// developer chose. A wrapper that binds `req`/`body` on `Span::call_site()` and
/// then extracts a handler parameter the developer spelled `body` produces two
/// bindings of the *same* name: the second masks the first, and every later
/// statement silently reads the wrong one. The compiler blames the attribute,
/// never the parameter — see the `#[routes]` regression in
/// `nest-rs-http/tests/integration/route_decorators.rs`.
///
/// A prefix convention (`__nestrs_body`) only narrows the window; hygiene closes
/// it, and lets the emitted code keep readable names under `cargo expand`.
///
/// Struct fields and method names are matched by spelling, not hygiene — keep
/// those on `Span::call_site()`.
pub fn mixed_site_ident(name: &str) -> Ident {
    Ident::new(name, proc_macro2::Span::mixed_site())
}

/// Rewrite each value argument's pattern to the plain identifier it forwards
/// under, so the sequence can serve as a generated wrapper's parameter list.
/// Returns those identifiers in order, index-aligned with the value arguments.
///
/// Types are untouched: `Path(name): Path<String>` becomes `name:
/// Path<String>`, which is what lets the wrapper pass the whole extractor on to
/// a method that destructures it itself. See [`forwarded_idents`] for the why.
pub fn normalize_forwarded_args<'a>(
    inputs: impl IntoIterator<Item = &'a mut FnArg>,
) -> syn::Result<Vec<Ident>> {
    let mut idents = Vec::new();
    for arg in inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        let ident = binder_of(&pat_type.pat)?;
        // Keep the span of the pattern it replaces, so a later error about this
        // parameter still points at the developer's own code.
        *pat_type.pat = Pat::Ident(syn::PatIdent {
            attrs: Vec::new(),
            by_ref: None,
            mutability: None,
            ident: ident.clone(),
            subpat: None,
        });
        idents.push(ident);
    }
    Ok(idents)
}

/// The single identifier a parameter pattern binds — the name a generated
/// wrapper declares it under and forwards it by.
fn binder_of(pat: &Pat) -> syn::Result<Ident> {
    // The common case, and the only one that needs no rewrite.
    if let Pat::Ident(pat_ident) = pat
        && pat_ident.subpat.is_none()
    {
        return Ok(pat_ident.ident.clone());
    }
    let mut found = Vec::new();
    collect_binders(pat, &mut found);
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(syn::Error::new_spanned(
            pat,
            "this handler argument binds no name, so the generated wrapper has \
             nothing to forward — give it one (`arg: Path<String>`, or \
             `Path(name): Path<String>`)",
        )),
        n => Err(syn::Error::new_spanned(
            pat,
            format!(
                "this handler argument binds {n} names, so the generated wrapper \
                 cannot tell which one to forward — bind the whole extractor \
                 under one name (`arg: Path<(String, u32)>`) and destructure in \
                 the body, or destructure to a single binding \
                 (`Path(name): Path<String>`)"
            ),
        )),
    }
}

/// Every identifier a pattern binds, in source order.
fn collect_binders(pat: &Pat, out: &mut Vec<Ident>) {
    match pat {
        Pat::Ident(pi) => {
            out.push(pi.ident.clone());
            if let Some((_, sub)) = &pi.subpat {
                collect_binders(sub, out);
            }
        }
        Pat::TupleStruct(ts) => ts.elems.iter().for_each(|p| collect_binders(p, out)),
        Pat::Tuple(t) => t.elems.iter().for_each(|p| collect_binders(p, out)),
        Pat::Paren(p) => collect_binders(&p.pat, out),
        Pat::Struct(s) => s.fields.iter().for_each(|f| collect_binders(&f.pat, out)),
        Pat::Reference(r) => collect_binders(&r.pat, out),
        Pat::Slice(s) => s.elems.iter().for_each(|p| collect_binders(p, out)),
        Pat::Or(o) => o.cases.iter().for_each(|p| collect_binders(p, out)),
        Pat::Type(t) => collect_binders(&t.pat, out),
        // `_`, a literal, a path constant: binds nothing.
        _ => {}
    }
}

/// The `TypeId` **and** the diagnostic label of each type a provider resolves
/// from the container outside its `#[inject]` fields — guards, filters,
/// interceptors, resolver `#[field_resolver]` `&Service` deps.
///
/// Both halves come out of **one** walk, deduplicated by token text once, so
/// they are index-aligned by construction. That alignment is load-bearing: the
/// access graph pairs `injected()[i]` with `injected_names()[i]` to name a
/// dependency no module provides. Two independent walks over the same list
/// would have to dedupe on the byte-identical rule forever, and a divergence
/// would not fail — it would silently make the boot error name the *wrong*
/// type, which is worse than the `<unnamed dependency>` placeholder it
/// replaces.
pub struct LayerDeps {
    /// `TypeId::of::<P>()` per layer, for `Discoverable::injected`.
    pub keys: Vec<TokenStream2>,
    /// The matching label per layer, for `Discoverable::injected_names`.
    pub labels: Vec<TokenStream2>,
}

/// Walk `items` once, yielding [`LayerDeps`]. Feeding its `keys` into
/// `Discoverable::injected` is what puts a layer under the access contract.
pub fn layer_deps<'a, T: ToTokens + 'a>(items: impl IntoIterator<Item = &'a T>) -> LayerDeps {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    let mut labels = Vec::new();
    for item in items {
        if !seen.insert(quote!(#item).to_string()) {
            continue;
        }
        keys.push(quote! { ::core::any::TypeId::of::<#item>() });
        let label = layer_label(item);
        labels.push(quote! { #label });
    }
    LayerDeps { keys, labels }
}

/// Short diagnostic name for a layer, through the crate's one labeller. Routed
/// via `Type` rather than `Path` so a `&Service` or `Arc<dyn Trait>` field
/// dependency — which `#[resolver]` passes here — reads as `dyn Trait` instead
/// of raw token text.
fn layer_label(item: &impl ToTokens) -> String {
    match syn::parse2::<syn::Type>(item.to_token_stream()) {
        Ok(ty) => type_label(&ty),
        Err(_) => quote!(#item).to_string(),
    }
}

/// `::std::vec![...]` of `#[inject]` dependency `TypeId`s — body for
/// [`dependencies_method`]/[`injected_method`] and for the inherent
/// `__nestrs_injected()` a struct decorator emits.
pub(crate) fn injected_keys_expr(dep_keys: &[TokenStream2]) -> TokenStream2 {
    quote! { ::std::vec![ #(#dep_keys),* ] }
}

/// `injected_keys_expr` extended with the dedup'd struct-level guard/filter/
/// interceptor `TypeId`s; the companion impl-block macro appends per-route/
/// per-message layers on top via [`injected_methods_with_layers`].
pub fn injected_keys_with_layers(dep_keys: &[TokenStream2], layers: &LayerDeps) -> TokenStream2 {
    let mut keys = dep_keys.to_vec();
    keys.extend(layers.keys.iter().cloned());
    injected_keys_expr(&keys)
}

/// The name half of [`injected_keys_with_layers`], index-aligned with it —
/// body for the inherent `__nestrs_injected_names()` a struct decorator emits.
/// Both take the same [`LayerDeps`], so the alignment is not a convention the
/// call site has to honour.
pub fn injected_names_with_layers(dep_names: &[TokenStream2], layers: &LayerDeps) -> TokenStream2 {
    let mut names = dep_names.to_vec();
    names.extend(layers.labels.iter().cloned());
    injected_keys_expr(&names)
}

/// `Discoverable::injected` **and** `injected_names` for an impl-block macro:
/// take the struct's `__nestrs_injected()` / `__nestrs_injected_names()` and
/// extend each with the per-route / per-message layers.
///
/// One function emitting both, over one [`LayerDeps`] — so an impl-block
/// decorator cannot append a key without its label, and adding a seventh layer
/// family to a call site's selector cannot misalign the two. The fixed-size,
/// explicitly-typed arrays keep `extend` unambiguous when no per-method layers
/// are present.
pub fn injected_methods_with_layers(
    self_ty: &impl quote::ToTokens,
    layers: &LayerDeps,
) -> TokenStream2 {
    let count = proc_macro2::Literal::usize_unsuffixed(layers.keys.len());
    let keys = &layers.keys;
    let labels = &layers.labels;
    quote! {
        fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
            let mut __keys = <#self_ty>::__nestrs_injected();
            let __layers: [::core::any::TypeId; #count] = [ #(#keys),* ];
            __keys.extend(__layers);
            __keys
        }

        fn injected_names() -> ::std::vec::Vec<&'static str> {
            let mut __names = <#self_ty>::__nestrs_injected_names();
            let __layers: [&'static str; #count] = [ #(#labels),* ];
            __names.extend(__layers);
            __names
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

/// `Discoverable::injected_names` — index-aligned with
/// [`injected_method`](injected_method), so the access graph can name a
/// dependency no module provides. Every provider that emits `injected` should
/// emit this too; one that does not falls back to a placeholder name.
pub fn injected_names_method(dep_names: &[TokenStream2]) -> TokenStream2 {
    quote! {
        fn injected_names() -> ::std::vec::Vec<&'static str> {
            ::std::vec![ #(#dep_names),* ]
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
