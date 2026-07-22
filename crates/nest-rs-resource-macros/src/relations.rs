//! Auto-generated bridges for entities: a PK loader on the entity's service
//! (so other entities can resolve `belongs_to` references without each one
//! re-declaring the loader), trait impls connecting the entity to its loader,
//! the wire DTO, and `#[ComplexObject]` field resolvers on the wire DTO for
//! every exposed (`#[expose]`) relation.
//!
//! Emission lives at the entity's call site (e.g. `users/entity.rs`); paths
//! resolve relative to that scope. Absolute paths are used for framework
//! crates so the user does not need to `use` them in `entity.rs`.
//!
//! Phase 1 — `belongs_to`: emits one `#[ComplexObject]` field per exposed
//! `HasOne` plus the PK loader on the service.
//!
//! Phase 2 — `has_many`: emits one `#[ComplexObject]` field per exposed
//! `HasMany`. The FK-side dataloader (`by_<fk_col>`) and the matching
//! `RelatedTo<Parent>` impl are emitted by the **FK-owning** entity (the side
//! that declares `belongs_to`), keeping every emission local to one module.

use nest_rs_codegen::{last_segment_ident, pascal_case};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::attr::{RelationKind, ResourceField, ResourceModel, complexity_attr, is_uuid};

/// Default complexity expression for an auto-emitted `HasMany` field resolver.
/// The loader caps each parent's children at
/// [`RELATION_LOAD_CAP`](nest_rs_seaorm::RELATION_LOAD_CAP), so fanout is
/// bounded — but the query-cost estimate still multiplies the cost of selected
/// sub-fields by `10` per level to keep deep chains expensive. A
/// 3-deep chain `{ users { posts { comments { id } } } }` scores
/// `10 + 10*10 + 10*100 = 1000` (only the outermost level enters the document
/// total, so the result is exactly `10^3`). async-graphql checks
/// `complexity > limit` strictly, so a `max_complexity = 1000` admits this
/// canonical chain — pin `999` to reject it, or accept it as the chosen
/// upper bound. The factor is hard-wired across the framework; override per
/// relation with `#[expose(complexity = "…")]` when the realistic fanout
/// differs. The override expression may reference `child_complexity` and
/// pure literals only — it lands on a method with no GraphQL arguments, so
/// names like `first` would resolve as unbound identifiers in the generated
/// closure. Paginated relations are left unexposed (no `#[expose]`) and given a
/// hand-rolled `#[field_resolver]` carrying its own `#[graphql(complexity = …)]`.
pub(crate) const DEFAULT_HAS_MANY_COMPLEXITY: &str = "10 * child_complexity";

pub fn emit(model: &ResourceModel) -> syn::Result<TokenStream2> {
    let Some(service) = model.service.clone() else {
        if model.has_auto_relations() {
            return Err(syn::Error::new_spanned(
                &model.source_ident,
                "this entity declares an exposed relation but `#[expose(... service = …)]` is missing — add the service path so the macro can emit its PK dataloader and PkLoadable impl",
            ));
        }
        return Ok(TokenStream2::new());
    };

    let mut pks = model.fields.iter().filter(|f| f.is_pk);
    let pk = pks.next().ok_or_else(|| {
        syn::Error::new_spanned(
            &model.source_ident,
            "auto-relations need a `#[sea_orm(primary_key)]` column on the entity",
        )
    })?;
    if let Some(extra) = pks.next() {
        // Composite primary keys silently produced a single-column `by_id`
        // loader — wrong rows on lookup with no diagnostic. The fix needs a
        // tuple-key loader; refuse for now rather than ship a footgun.
        return Err(syn::Error::new_spanned(
            &extra.ident,
            "auto-relations on composite primary keys are not supported yet — write a hand-rolled `#[dataloader]` on the service and leave the relation fields unexposed (no `#[expose]`)",
        ));
    }

    let pk_loader_ident = format_ident!("{}ById", last_segment_ident(&service));
    let pk_loader_block = emit_pk_loader(model, &service, pk);
    let pk_trait_impl = emit_pk_loadable_impl(model, &pk_loader_ident);
    let fk_loaders = emit_fk_loaders(model, &service)?;
    let field_resolvers = emit_field_resolvers(model, pk)?;

    Ok(quote! {
        #pk_loader_block
        #pk_trait_impl
        #fk_loaders
        #field_resolvers
    })
}

fn live_rows_filter(model: &ResourceModel) -> TokenStream2 {
    if model.soft_delete {
        quote! { .filter(::nest_rs_seaorm::live_condition::<Entity>()) }
    } else {
        quote! {}
    }
}

/// `#[dataloader] impl <Service> { async fn by_id(&self, keys: &[Pk]) -> ... }`.
/// Read-scoped via the ambient `Ability` — every call goes through `Repo`.
fn emit_pk_loader(model: &ResourceModel, service: &syn::Path, pk: &ResourceField) -> TokenStream2 {
    let pk_ident = &pk.ident;
    let pk_ty = &pk.ty;
    let pk_col = pascal_case(pk_ident);
    let wire = &model.output_ident;
    let target_label = format!("loading {} by id", wire);
    let live = live_rows_filter(model);

    quote! {
        #[::nest_rs_resource::graphql::dataloader]
        impl #service {
            async fn by_id(
                &self,
                __keys: &[#pk_ty],
            ) -> ::core::result::Result<
                ::std::collections::HashMap<#pk_ty, #wire>,
                ::nest_rs_seaorm::ServiceError,
            > {
                if __keys.is_empty() {
                    return ::core::result::Result::Ok(::std::collections::HashMap::new());
                }
                ::nest_rs_resource::tracing::debug!(
                    target: "nest_rs::loader",
                    count = __keys.len(),
                    #target_label,
                );
                let __conn = ::nest_rs_seaorm::Repo::<Entity>::conn()?;
                let __rows = ::nest_rs_seaorm::Repo::<Entity>::scoped(
                    ::nest_rs_authz::Action::Read,
                )
                    #live
                    .filter(
                        <Column as ::sea_orm::ColumnTrait>::is_in(
                            &Column::#pk_col,
                            __keys.iter().cloned(),
                        ),
                    )
                    .all(&__conn)
                    .await?;
                // Row-level filtering happened above (`scoped(Read)`); apply
                // field-level masking here too, via the ambient ability the
                // batch runs under (`LoaderScope`) — a relation must not leak
                // columns the caller is not granted.
                let mut __map: ::std::collections::HashMap<#pk_ty, #wire> =
                    ::std::collections::HashMap::with_capacity(__rows.len());
                for __row in __rows {
                    let __wire = ::nest_rs_authz::masked_output_ambient::<
                        ::nest_rs_authz::Read,
                        Entity,
                        #wire,
                    >(&__row)
                    .map_err(|__e| ::nest_rs_seaorm::ServiceError::Masking(
                        ::std::string::ToString::to_string(&__e),
                    ))?;
                    __map.insert(__row.#pk_ident, __wire);
                }
                ::core::result::Result::Ok(__map)
            }
        }
    }
}

/// `impl PkLoadable for Entity { type Loader = <Service>ById; type Wire = User; }`
/// — the link an outside entity uses to resolve a `belongs_to` pointing here.
fn emit_pk_loadable_impl(model: &ResourceModel, loader: &Ident) -> TokenStream2 {
    let wire = &model.output_ident;
    quote! {
        impl ::nest_rs_resource::PkLoadable for Entity {
            type Loader = #loader;
            type Wire = #wire;
        }
    }
}

/// FK-side emission. For each exposed `belongs_to` (the FK-owning side knows
/// the column name + type), emits a `by_<fk_col>` batched loader on the
/// service plus an `impl RelatedTo<TargetEntity> for Entity` so the inverse
/// `has_many` field resolver on the target side can find this loader without
/// hard-coding the service name.
fn emit_fk_loaders(model: &ResourceModel, service: &syn::Path) -> syn::Result<TokenStream2> {
    let mut blocks = Vec::new();
    // Two `belongs_to` pointing at the same target would emit two
    // `impl RelatedTo<#target> for Entity` blocks — coherence error E0119
    // with a span deep in the macro expansion. The inverse `has_many` lookup
    // on the target side can only consume one, so the second is ambiguous
    // even if the FK loaders were both registered. Refuse it at parse time —
    // keyed by the target's module-qualified path, normalized so a leading
    // `crate::`/`self::` anchor doesn't split one entity across two spellings
    // (`orgs::Entity` vs `crate::orgs::Entity` collide here as a clear
    // diagnostic instead of surfacing later as an opaque duplicate-impl
    // `E0119`). NOTE: do *not* key by the last path segment — SeaORM entity
    // types are all named `Entity` (`users::Entity`, `orgs::Entity`), so the
    // last segment is always `Entity` and keying on it would false-positive
    // every second `belongs_to`; the *module* path identifies the parent.
    // Stored as `(key, full_spelling, field)` so an aliased collision can name
    // both spellings.
    let mut seen_targets: Vec<(String, String, &Ident)> = Vec::new();
    for field in &model.fields {
        if !field.read {
            continue;
        }
        let Some(RelationKind::BelongsTo { from, target, .. }) = &field.relation else {
            continue;
        };
        let target_spelling = target
            .segments
            .iter()
            .map(|seg| seg.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        // Normalize away a redundant leading `crate::`/`self::` anchor so
        // `crate::orgs::Entity` and `orgs::Entity` key alike, while
        // `orgs::Entity` and `users::Entity` stay distinct.
        let target_key = target_spelling
            .strip_prefix("crate::")
            .or_else(|| target_spelling.strip_prefix("self::"))
            .unwrap_or(&target_spelling)
            .to_owned();
        if let Some((_, prev_spelling, prev_field)) =
            seen_targets.iter().find(|(key, _, _)| key == &target_key)
        {
            // Same tail via two *different* spellings ⇒ name both, so the
            // developer sees these are one entity reached by aliased paths.
            let aliased = if prev_spelling == &target_spelling {
                String::new()
            } else {
                format!(" (`{prev_spelling}` and `{target_spelling}` name the same entity)")
            };
            return Err(syn::Error::new_spanned(
                &field.ident,
                format!(
                    "two `belongs_to` relations targeting the same parent are not supported (clashes with `{prev_field}`){aliased}; leave one unexposed (no `#[expose]`) and write a hand-rolled `#[field_resolver]`",
                ),
            ));
        }
        seen_targets.push((target_key, target_spelling, &field.ident));

        let fk_field = model.fields.iter().find(|f| &f.ident == from).ok_or_else(|| {
            syn::Error::new_spanned(
                &field.ident,
                format!(
                    "`belongs_to` declares `from = \"{}\"` but no column with that name is exposed on this entity",
                    from,
                ),
            )
        })?;
        let fk_ty = &fk_field.ty;
        let fk_col_pascal = pascal_case(from);
        let method_name = format_ident!("by_{}", from);
        let loader_ident = format_ident!("{}By{}", last_segment_ident(service), fk_col_pascal,);
        let wire = &model.output_ident;
        let target_label = format!("loading {} by {}", wire, from);
        let live = live_rows_filter(model);

        blocks.push(quote! {
            #[::nest_rs_resource::graphql::dataloader]
            impl #service {
                async fn #method_name(
                    &self,
                    __keys: &[#fk_ty],
                ) -> ::core::result::Result<
                    ::std::collections::HashMap<#fk_ty, ::std::vec::Vec<#wire>>,
                    ::nest_rs_seaorm::ServiceError,
                > {
                    if __keys.is_empty() {
                        return ::core::result::Result::Ok(::std::collections::HashMap::new());
                    }
                    ::nest_rs_resource::tracing::debug!(
                        target: "nest_rs::loader",
                        count = __keys.len(),
                        #target_label,
                    );
                    // Bound memory: never load more than `RELATION_LOAD_CAP`
                    // children per parent. The batch query is capped at
                    // `cap × keys` and each parent's bucket is truncated, so an
                    // unbounded relation (millions of children under one parent)
                    // can never pull an unbounded result set into memory.
                    use ::sea_orm::{QueryOrder as _, QuerySelect as _};
                    let __cap = ::nest_rs_seaorm::RELATION_LOAD_CAP;
                    let __limit = __cap.saturating_mul(__keys.len() as u64);
                    let __conn = ::nest_rs_seaorm::Repo::<Entity>::conn()?;
                    let __rows = ::nest_rs_seaorm::Repo::<Entity>::scoped(
                        ::nest_rs_authz::Action::Read,
                    )
                        #live
                        .filter(
                            <Column as ::sea_orm::ColumnTrait>::is_in(
                                &Column::#fk_col_pascal,
                                __keys.iter().cloned(),
                            ),
                        )
                        // Group children of a parent contiguously so the batch
                        // cap truncates whole parents, not an arbitrary slice.
                        .order_by_asc(Column::#fk_col_pascal)
                        .limit(__limit)
                        .all(&__conn)
                        .await?;
                    // Global-cap saturation ⇒ possible cross-parent STARVATION:
                    // the `cap × keys` limit ordered by FK asc can be consumed by
                    // early (low-FK) parents, leaving later parents with empty
                    // buckets that are returned as `[]` — indistinguishable from
                    // "no children". That silent wrong answer (DATA-R2) must be
                    // surfaced loudly; the per-bucket truncation warn below only
                    // catches a single parent *exceeding* the cap, never starved
                    // siblings. (The real fix — a per-parent `ROW_NUMBER()` limit —
                    // rides the roadmapped HasMany `Connection<T>` pagination.)
                    let __saturated = __rows.len() as u64 == __limit;
                    let mut __raw: ::std::collections::HashMap<#fk_ty, ::std::vec::Vec<Model>> =
                        __keys
                            .iter()
                            .map(|__k| (::core::clone::Clone::clone(__k), ::std::vec::Vec::new()))
                            .collect();
                    for __row in __rows {
                        if let ::core::option::Option::Some(__bucket) = __raw.get_mut(&__row.#from) {
                            __bucket.push(__row);
                        }
                    }
                    let mut __truncated = false;
                    let mut __map: ::std::collections::HashMap<#fk_ty, ::std::vec::Vec<#wire>> =
                        ::std::collections::HashMap::with_capacity(__raw.len());
                    for (__k, mut __children) in __raw {
                        if __children.len() as u64 > __cap {
                            __children.truncate(__cap as usize);
                            __truncated = true;
                        }
                        let mut __wire = ::std::vec::Vec::with_capacity(__children.len());
                        for __row in &__children {
                            // Field-level masking through the ambient ability,
                            // mirroring `by_id` — `scoped(Read)` only filters
                            // rows, not columns.
                            __wire.push(
                                ::nest_rs_authz::masked_output_ambient::<
                                    ::nest_rs_authz::Read,
                                    Entity,
                                    #wire,
                                >(__row)
                                .map_err(|__e| ::nest_rs_seaorm::ServiceError::Masking(
                                    ::std::string::ToString::to_string(&__e),
                                ))?,
                            );
                        }
                        __map.insert(__k, __wire);
                    }
                    if __truncated || __saturated {
                        ::nest_rs_resource::tracing::warn!(
                            target: "nest_rs::loader",
                            cap = __cap,
                            loader = #target_label,
                            saturated = __saturated,
                            "relation batch hit RELATION_LOAD_CAP — children were truncated, and \
                             when `saturated` some parents may be MISSING children entirely \
                             (starved); narrow the query or paginate this relation",
                        );
                    }
                    ::core::result::Result::Ok(__map)
                }
            }

            impl ::nest_rs_resource::RelatedTo<#target> for Entity {
                type Loader = #loader_ident;
                type Wire = #wire;
            }
        });
    }
    if blocks.is_empty() {
        return Ok(TokenStream2::new());
    }
    Ok(quote! { #(#blocks)* })
}

/// `#[ComplexObject] impl <Wire> { … }` — one method per exposed relation.
/// `BelongsTo` → `Option<TargetWire>` via `PkLoadable`. `HasMany` →
/// `Vec<TargetWire>` via `RelatedTo<Self::Entity>`.
fn emit_field_resolvers(model: &ResourceModel, pk: &ResourceField) -> syn::Result<TokenStream2> {
    let mut methods = Vec::new();
    for field in &model.fields {
        if !field.read {
            continue;
        }
        let Some(kind) = &field.relation else {
            continue;
        };
        match kind {
            RelationKind::BelongsTo { from, target, .. } => {
                methods.push(emit_belongs_to_method(model, field, from, target)?);
            }
            RelationKind::HasMany { target, .. } => {
                methods.push(emit_has_many_method(field, target, pk)?);
            }
        }
    }
    if methods.is_empty() {
        return Ok(TokenStream2::new());
    }
    let wire = &model.output_ident;
    Ok(quote! {
        #[::nest_rs_resource::graphql::async_graphql::ComplexObject]
        impl #wire {
            #(#methods)*
        }
    })
}

/// One BelongsTo field resolver: load the parent's FK column via the target
/// entity's PK loader, returning its wire DTO. Default complexity
/// (async-graphql's `1 + child_complexity`) tracks the upper bound — one
/// parent row loaded plus the cost of the selected sub-fields. A row denied
/// by the ambient `Ability` resolves to `None` for free, so the actual mean
/// cost is `0..=1` rows; we keep the default rather than over-penalising
/// ability-heavy schemas.
fn emit_belongs_to_method(
    model: &ResourceModel,
    field: &ResourceField,
    fk: &Ident,
    target: &syn::Path,
) -> syn::Result<TokenStream2> {
    let name = &field.ident;
    let fk_field = model.fields.iter().find(|f| &f.ident == fk).ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            format!(
                "`belongs_to` declares `from = \"{}\"` but no column with that name is exposed on this entity",
                fk,
            ),
        )
    })?;

    let key_expr = wire_key_expr(&fk_field.ty, fk);
    let complexity = complexity_attr(&field.complexity, None);

    Ok(quote! {
        #complexity
        async fn #name(
            &self,
            __ctx: &::nest_rs_resource::graphql::async_graphql::Context<'_>,
        ) -> ::nest_rs_resource::graphql::async_graphql::Result<
            ::core::option::Option<<#target as ::nest_rs_resource::PkLoadable>::Wire>,
        > {
            // `data_opt` + error, never `data_unchecked` (which panics): an
            // unseeded loader — its owner service's module is unreachable from
            // this app — degrades to a GraphQL error naming the relation, not a
            // request-time panic. Boot already warns (`warn_unreachable_loaders`).
            let __loader = __ctx
                .data_opt::<
                    ::nest_rs_resource::graphql::async_graphql::dataloader::DataLoader<
                        <#target as ::nest_rs_resource::PkLoadable>::Loader,
                    >,
                >()
                .ok_or_else(|| {
                    ::nest_rs_resource::graphql::async_graphql::Error::new(::std::format!(
                        "relation `{}` is exposed but its dataloader `{}` is not seeded — the module providing it is not imported by (or reachable from) this app",
                        ::core::stringify!(#name),
                        ::core::any::type_name::<
                            ::nest_rs_resource::graphql::async_graphql::dataloader::DataLoader<
                                <#target as ::nest_rs_resource::PkLoadable>::Loader,
                            >,
                        >(),
                    ))
                })?;
            let __key = #key_expr;
            ::core::result::Result::Ok(__loader.load_one(__key).await?)
        }
    })
}

/// One HasMany field resolver: load the children of `self` via the target's
/// `RelatedTo<Self::Entity>::Loader`, keyed on `self`'s PK. The target's macro
/// is responsible for declaring the `RelatedTo` impl from its own `belongs_to`.
///
/// The auto-emitted loader returns up to
/// [`RELATION_LOAD_CAP`](nest_rs_seaorm::RELATION_LOAD_CAP) children per parent
/// (a hard memory backstop). We **always** emit a `#[graphql(complexity = …)]`
/// override so the
/// score scales multiplicatively (`factor * child_complexity`) instead of
/// additively (`1 + child_complexity`, async-graphql's default for bare
/// fields). That asymmetry is the whole point: BelongsTo loads one row,
/// HasMany loads N. Override with `#[expose(complexity = …)]` for a tighter
/// or looser factor — but only `child_complexity` and pure literals; the
/// resolver method takes no GraphQL args, so referencing `first` or any
/// other argument name from the override produces a confusing
/// "cannot find value `first`" error in the generated closure.
fn emit_has_many_method(
    field: &ResourceField,
    target: &syn::Path,
    pk: &ResourceField,
) -> syn::Result<TokenStream2> {
    let name = &field.ident;
    let key_expr = wire_key_expr(&pk.ty, &pk.ident);
    let complexity = complexity_attr(&field.complexity, Some(DEFAULT_HAS_MANY_COMPLEXITY));

    Ok(quote! {
        #complexity
        async fn #name(
            &self,
            __ctx: &::nest_rs_resource::graphql::async_graphql::Context<'_>,
        ) -> ::nest_rs_resource::graphql::async_graphql::Result<
            ::std::vec::Vec<<#target as ::nest_rs_resource::RelatedTo<Entity>>::Wire>,
        > {
            // `data_opt` + error, never `data_unchecked` (which panics): an
            // unseeded loader — its owner service's module is unreachable from
            // this app — degrades to a GraphQL error naming the relation, not a
            // request-time panic. Boot already warns (`warn_unreachable_loaders`).
            let __loader = __ctx
                .data_opt::<
                    ::nest_rs_resource::graphql::async_graphql::dataloader::DataLoader<
                        <#target as ::nest_rs_resource::RelatedTo<Entity>>::Loader,
                    >,
                >()
                .ok_or_else(|| {
                    ::nest_rs_resource::graphql::async_graphql::Error::new(::std::format!(
                        "relation `{}` is exposed but its dataloader `{}` is not seeded — the module providing it is not imported by (or reachable from) this app",
                        ::core::stringify!(#name),
                        ::core::any::type_name::<
                            ::nest_rs_resource::graphql::async_graphql::dataloader::DataLoader<
                                <#target as ::nest_rs_resource::RelatedTo<Entity>>::Loader,
                            >,
                        >(),
                    ))
                })?;
            let __key = #key_expr;
            ::core::result::Result::Ok(
                __loader.load_one(__key).await?.unwrap_or_default(),
            )
        }
    })
}

/// The wire representation of a column → key the dataloader expects. `Uuid`
/// projects as `String` on the wire (see `dto.rs`), so the resolver parses
/// it back; other types pass through cloned.
fn wire_key_expr(ty: &Type, ident: &Ident) -> TokenStream2 {
    if is_uuid(ty) {
        quote! {
            ::uuid::Uuid::parse_str(&self.#ident)
                .map_err(|__e| ::nest_rs_resource::graphql::async_graphql::Error::new(__e.to_string()))?
        }
    } else {
        quote! { ::core::clone::Clone::clone(&self.#ident) }
    }
}
