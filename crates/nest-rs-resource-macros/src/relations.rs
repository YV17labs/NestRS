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
//! `HasMany`, returning a Relay `Connection`. The FK-side dataloader
//! (`by_<fk_col>`) and the matching `RelatedTo<Parent, Via>` impl are emitted by
//! the **FK-owning** entity (the side that declares `belongs_to`), keeping every
//! emission local to one module.

use nest_rs_codegen::{last_segment_ident, pascal_case};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::attr::{
    RelationKind, ResourceField, ResourceModel, complexity_attr, graphql_root, graphql_root_str,
    is_uuid,
};

/// Default complexity expression for an auto-emitted `HasMany` field resolver.
///
/// The field now *takes* its page size, so the estimate is no longer a guessed
/// constant: it is the number of children the client asked for, times the cost
/// of each. A query asking `first: 5` pays a twentieth of one asking `first:
/// 100`, which is the whole point of a complexity ceiling and was not
/// expressible while the field returned an unparameterised list. A 3-deep chain
/// at the default page size scores `20^3`; at `first: 3` it scores `27`.
///
/// The `20` mirrors [`DEFAULT_PAGE_SIZE`](nest_rs_seaorm::DEFAULT_PAGE_SIZE),
/// which the emitted body reaches by path. It is spelled again here because
/// async-graphql takes a complexity expression as a **string**, which no
/// re-rooting pass rewrites. The duplication is bounded on purpose: this literal
/// only estimates a cost, so a drift shifts a score and can never change a
/// result.
///
/// **The clamp is the part that is not merely an estimate.** `first` reaches
/// this expression exactly as the client sent it: async-graphql's `u64` scalar
/// advertises `Int` in the SDL but parses the whole `u64` range, and
/// `clamp_page_size`'s `1..=100` window lives inside the resolver *body*, which
/// the estimate never enters. So `first: 18446744073709551615` multiplied
/// straight through — a `multiply with overflow` panic in a debug build (and
/// there is no `CatchPanic` on the HTTP transport), and in release a wrap to a
/// tiny score that slips the field under `max_complexity` entirely.
///
/// It is spelled as arithmetic rather than as a call for the same reason the
/// `20` is a literal: this is a **string**, so no path in it is re-rooted, and
/// `nest-rs-resource`'s own tests compile the expansion without the umbrella in
/// scope. The window mirrors
/// [`clamp_page_size`](nest_rs_seaorm::clamp_page_size) and is bounded
/// duplication of the same kind — with the overflow gone, a drift here shifts a
/// score and cannot change a result.
///
/// async-graphql checks `complexity > limit` strictly. Override per relation
/// with `#[expose(complexity = "…")]`; the expression may reference
/// `child_complexity`, pure literals, **and the field's own `first` argument**
/// (`Option<u64>`), which the previous unparameterised resolver could not offer.
pub(crate) const DEFAULT_HAS_MANY_COMPLEXITY: &str =
    "first.unwrap_or(20).clamp(1, 100) as usize * child_complexity";

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
    let fk_loaders = emit_fk_loaders(model, &service, pk)?;
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

/// The same live-row predicate as a `Condition` value, for the `Repo` methods
/// that take one rather than being chained onto.
fn live_rows_condition(model: &ResourceModel) -> TokenStream2 {
    if model.soft_delete {
        quote! { ::nest_rs_seaorm::live_condition::<Entity>() }
    } else {
        quote! { ::nest_rs_resource::sea_orm::Condition::all() }
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
                        <Column as ::nest_rs_resource::sea_orm::ColumnTrait>::is_in(
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

/// The scalar column a `belongs_to` names in `from = "…"`, or the refusal both
/// emission sites raise when the entity has no such column — one lookup, one
/// sentence, so the two cannot come to word it differently. Spanned at the
/// relation field, which is where the developer wrote the name.
///
/// Exposure is deliberately **not** required here: a loader keys on the
/// entity's `Column`, which exists whether or not the column crosses the wire.
fn fk_column<'a>(
    model: &'a ResourceModel,
    relation: &Ident,
    fk: &Ident,
) -> syn::Result<&'a ResourceField> {
    model.fields.iter().find(|f| &f.ident == fk).ok_or_else(|| {
        syn::Error::new_spanned(
            relation,
            format!(
                "`belongs_to` declares `from = \"{fk}\"` but this entity has no column with that name",
            ),
        )
    })
}

/// The same column, additionally required to carry `#[expose]`.
///
/// The `#[ComplexObject]` field resolver reads the key off the **wire object**
/// (`self.<fk>`), and the wire object holds exposed columns only — so a hidden
/// foreign key passed the lookup above and then failed as `no field `org_id` on
/// type `Post``, pointing inside the expansion at a struct the developer never
/// wrote. Refuse it here, where the two attributes that disagree are both in
/// view.
fn exposed_fk_column<'a>(
    model: &'a ResourceModel,
    relation: &Ident,
    fk: &Ident,
) -> syn::Result<&'a ResourceField> {
    let column = fk_column(model, relation, fk)?;
    if !column.read {
        return Err(syn::Error::new_spanned(
            relation,
            format!(
                "`belongs_to` declares `from = \"{fk}\"`, but that column carries no `#[expose]` — this relation resolves by reading the key off the wire object, so the foreign key has to cross the wire too. Expose the column, or leave the relation unexposed",
            ),
        ));
    }
    Ok(column)
}

/// FK-side emission. For each exposed `belongs_to` (the FK-owning side knows
/// the column name + type), emits a `by_<fk_col>` batched loader on the
/// service plus an `impl RelatedTo<TargetEntity> for Entity` so the inverse
/// `has_many` field resolver on the target side can find this loader without
/// hard-coding the service name.
fn emit_fk_loaders(
    model: &ResourceModel,
    service: &syn::Path,
    pk: &ResourceField,
) -> syn::Result<TokenStream2> {
    let mut blocks = Vec::new();
    // How many exposed `belongs_to` point at each parent. A parent named once
    // gets the `SoleForeignKey` impl — the default a `HasMany` takes when it
    // names no column. A parent named twice gets none, because two
    // `impl RelatedTo<#target> for Entity` blocks are coherence error E0119
    // with a span deep in the expansion; the inverse side must then say which
    // key it follows with `#[expose(via = "…")]`, and the `on_unimplemented`
    // note on `RelatedTo` tells it so.
    //
    // Keyed by the target's module-qualified path, normalized so a leading
    // `crate::`/`self::` anchor doesn't split one entity across two spellings.
    // NOTE: do *not* key by the last path segment — SeaORM entity types are all
    // named `Entity` (`users::Entity`, `orgs::Entity`), so the last segment is
    // always `Entity`; the *module* path identifies the parent.
    let mut target_counts: Vec<(String, usize)> = Vec::new();
    for field in &model.fields {
        if !field.read {
            continue;
        }
        let Some(RelationKind::BelongsTo { target, .. }) = &field.relation else {
            continue;
        };
        let key = target_key(target);
        match target_counts.iter_mut().find(|(k, _)| k == &key) {
            Some((_, count)) => *count += 1,
            None => target_counts.push((key, 1)),
        }
    }

    for field in &model.fields {
        if !field.read {
            continue;
        }
        let Some(RelationKind::BelongsTo { from, target, .. }) = &field.relation else {
            continue;
        };
        let key = target_key(target);
        let sole = target_counts
            .iter()
            .find(|(k, _)| k == &key)
            .is_some_and(|(_, count)| *count == 1);

        let fk_ty = &fk_column(model, &field.ident, from)?.ty;
        let fk_col_pascal = pascal_case(from);
        let method_name = format_ident!("by_{}", from);
        let loader_ident = format_ident!("{}By{}", last_segment_ident(service), fk_col_pascal,);
        let wire = &model.output_ident;
        let via_ident = via_marker_ident(from);
        let via_doc = format!(
            "`#[expose(via = \"{from}\")]` resolved to a type. Emitted beside this entity by \
             `#[expose]`, one per `belongs_to`, so the *parent* side of a relation can name a \
             foreign-key column — which is not otherwise a thing Rust can name. Never written \
             by hand: the developer writes the column string.",
        );
        // The `SoleForeignKey` impl exists only while this entity points at
        // `#target` once. A second `belongs_to` at the same parent removes it,
        // so the inverse `HasMany` stops compiling until it names a column —
        // rather than silently resolving through whichever key came first.
        let sole_impl = sole.then(|| {
            quote! {
                impl ::nest_rs_resource::RelatedTo<#target> for Entity {
                    type Loader = #loader_ident;
                    type Wire = #wire;
                }
            }
        });
        let target_label = format!("paging {} by {}", wire, from);
        let live = live_rows_condition(model);
        let pk_ident = &pk.ident;

        blocks.push(quote! {
            #[::nest_rs_resource::graphql::dataloader]
            impl #service {
                async fn #method_name(
                    &self,
                    __keys: &[::nest_rs_resource::RelationKey<#fk_ty>],
                ) -> ::core::result::Result<
                    ::std::collections::HashMap<
                        ::nest_rs_resource::RelationKey<#fk_ty>,
                        ::nest_rs_resource::RelationPage<#wire>,
                    >,
                    ::nest_rs_seaorm::ServiceError,
                > {
                    let mut __out: ::std::collections::HashMap<
                        ::nest_rs_resource::RelationKey<#fk_ty>,
                        ::nest_rs_resource::RelationPage<#wire>,
                    > = ::std::collections::HashMap::with_capacity(__keys.len());
                    if __keys.is_empty() {
                        return ::core::result::Result::Ok(__out);
                    }
                    ::nest_rs_resource::tracing::debug!(
                        target: "nest_rs::loader",
                        count = __keys.len(),
                        #target_label,
                    );
                    // Siblings of one selection share a window, so this is one
                    // group in practice — but two aliases of the same relation
                    // may ask for different pages, and serving one parent's page
                    // to both is the bug the key's window exists to prevent.
                    let mut __windows: ::std::vec::Vec<(
                        u64,
                        ::core::option::Option<::nest_rs_resource::uuid::Uuid>,
                        ::std::vec::Vec<#fk_ty>,
                    )> = ::std::vec::Vec::new();
                    for __key in __keys {
                        match __windows
                            .iter_mut()
                            .find(|(__first, __after, _)| *__first == __key.first && *__after == __key.after)
                        {
                            ::core::option::Option::Some((_, _, __parents)) => {
                                __parents.push(::core::clone::Clone::clone(&__key.parent));
                            }
                            ::core::option::Option::None => __windows.push((
                                __key.first,
                                __key.after,
                                ::std::vec![::core::clone::Clone::clone(&__key.parent)],
                            )),
                        }
                    }

                    for (__first, __after, __parents) in __windows {
                        // One round trip per window, ranked per parent — the
                        // `WHERE fk IN (…) LIMIT n` shape this replaces could
                        // starve later parents into an empty list that read as
                        // "no children" (DATA-R2).
                        let __pages = ::nest_rs_seaorm::Repo::<Entity>::relation_pages(
                            Column::#fk_col_pascal,
                            &__parents,
                            __first,
                            __after,
                            #live,
                        )
                        .await?;
                        for (__parent, __page) in __pages {
                            let mut __edges = ::std::vec::Vec::with_capacity(__page.items.len());
                            for __row in &__page.items {
                                // Field-level masking through the ambient
                                // ability, mirroring `by_id` — `scoped(Read)`
                                // only filters rows, not columns.
                                let __wire = ::nest_rs_authz::masked_output_ambient::<
                                    ::nest_rs_authz::Read,
                                    Entity,
                                    #wire,
                                >(__row)
                                .map_err(|__e| ::nest_rs_seaorm::ServiceError::Masking(
                                    ::std::string::ToString::to_string(&__e),
                                ))?;
                                __edges.push((
                                    ::std::string::ToString::to_string(&__row.#pk_ident),
                                    __wire,
                                ));
                            }
                            __out.insert(
                                ::nest_rs_resource::RelationKey {
                                    parent: __parent,
                                    first: __first,
                                    after: __after,
                                },
                                ::nest_rs_resource::RelationPage {
                                    edges: __edges,
                                    has_next_page: __page.has_more,
                                },
                            );
                        }
                    }
                    ::core::result::Result::Ok(__out)
                }
            }

            #[doc = #via_doc]
            #[doc(hidden)]
            pub struct #via_ident;

            impl ::nest_rs_resource::RelatedTo<#target, #via_ident> for Entity {
                type Loader = #loader_ident;
                type Wire = #wire;
            }

            #sole_impl
        });
    }
    if blocks.is_empty() {
        return Ok(TokenStream2::new());
    }
    Ok(quote! { #(#blocks)* })
}

/// The parent-facing name of a foreign-key column: `author_id` → `ByAuthorId`.
/// A zero-sized marker emitted beside the child entity, because `RelatedTo`
/// needs a *type* to distinguish two keys and a column name is a string.
fn via_marker_ident(column: &Ident) -> Ident {
    format_ident!("By{}", pascal_case(column), span = column.span())
}

/// `RelatedTo<Entity, Via>` for one `HasMany`, as the trait-path half of a
/// `<Child as …>::Loader` projection.
///
/// Without `via` the default [`SoleForeignKey`](nest_rs_resource::SoleForeignKey)
/// applies and the path is written bare.
///
/// With it, the marker is reached **beside the child entity the developer
/// wrote**: `HasMany<crate::posts::Entity>` + `via = "author_id"` resolves to
/// `crate::posts::ByAuthorId`. That module is the only place both sides of the
/// relation can name — the child's macro emits the marker there, and the parent
/// knows the path it typed. So a child module re-exporting its entity must carry
/// the marker along with it; a `pub use entity::*` (the scaffolded shape) does,
/// and anything narrower reports as a plain unresolved path naming the marker.
fn related_to_path(target: &syn::Path, via: Option<&syn::LitStr>) -> syn::Result<TokenStream2> {
    let Some(via) = via else {
        return Ok(quote! { ::nest_rs_resource::RelatedTo<Entity> });
    };
    let column: Ident = via.parse()?;
    let mut marker = target.clone();
    let Some(last) = marker.segments.last_mut() else {
        return Err(syn::Error::new_spanned(
            target,
            "`HasMany<T>` needs a path to the child entity for `via` to resolve against",
        ));
    };
    last.ident = via_marker_ident(&column);
    last.arguments = syn::PathArguments::None;
    Ok(quote! { ::nest_rs_resource::RelatedTo<Entity, #marker> })
}

/// The parent entity path, normalized for counting: a redundant leading
/// `crate::`/`self::` anchor is stripped so `crate::orgs::Entity` and
/// `orgs::Entity` count as one parent, while `orgs::Entity` and `users::Entity`
/// stay distinct.
fn target_key(target: &syn::Path) -> String {
    let spelling = target
        .segments
        .iter()
        .map(|seg| seg.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    spelling
        .strip_prefix("crate::")
        .or_else(|| spelling.strip_prefix("self::"))
        .unwrap_or(&spelling)
        .to_owned()
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
            RelationKind::HasMany { target, via } => {
                methods.push(emit_has_many_method(field, target, via.as_ref(), pk)?);
            }
        }
    }
    if methods.is_empty() {
        return Ok(TokenStream2::new());
    }
    let wire = &model.output_ident;
    let root = graphql_root();
    let root_str = graphql_root_str();
    Ok(quote! {
        #[#root::ComplexObject(crate = #root_str)]
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
    let fk_field = exposed_fk_column(model, name, fk)?;

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

/// One HasMany field resolver: one page of the children of `self`, through the
/// target's `RelatedTo<Self::Entity, Via>::Loader`, keyed on `self`'s PK. The
/// target's macro is responsible for declaring the `RelatedTo` impl from its own
/// `belongs_to`.
///
/// The field is a Relay `Connection`: `first` / `after` in, `edges { cursor node }`
/// and `pageInfo` out, with the cursor being the child's own primary key — so
/// the same keyset the rest of the framework pages by. `first` is clamped by
/// `clamp_page_size`, which is what bounds fanout now that no hard per-parent
/// cap does; a relation over millions of rows is walked a page at a time
/// instead of truncated at 100 with a `warn`.
///
/// We **always** emit a `#[graphql(complexity = …)]` override so the score
/// scales with the page asked for rather than additively
/// (`1 + child_complexity`, async-graphql's default for bare fields). That
/// asymmetry is the whole point: BelongsTo loads one row, HasMany loads a page.
/// Override with `#[expose(complexity = …)]`, which may now reference `first`.
fn emit_has_many_method(
    field: &ResourceField,
    target: &syn::Path,
    via: Option<&syn::LitStr>,
    pk: &ResourceField,
) -> syn::Result<TokenStream2> {
    let name = &field.ident;
    let key_expr = wire_key_expr(&pk.ty, &pk.ident);
    let complexity = complexity_attr(&field.complexity, Some(DEFAULT_HAS_MANY_COMPLEXITY));
    let related = related_to_path(target, via)?;

    Ok(quote! {
        #complexity
        async fn #name(
            &self,
            __ctx: &::nest_rs_resource::graphql::async_graphql::Context<'_>,
            first: ::core::option::Option<u64>,
            after: ::core::option::Option<::std::string::String>,
        ) -> ::nest_rs_resource::graphql::async_graphql::Result<
            ::nest_rs_resource::graphql::async_graphql::connection::Connection<
                ::std::string::String,
                <#target as #related>::Wire,
            >,
        > {
            // `data_opt` + error, never `data_unchecked` (which panics): an
            // unseeded loader — its owner service's module is unreachable from
            // this app — degrades to a GraphQL error naming the relation, not a
            // request-time panic. Boot already warns (`warn_unreachable_loaders`).
            let __loader = __ctx
                .data_opt::<
                    ::nest_rs_resource::graphql::async_graphql::dataloader::DataLoader<
                        <#target as #related>::Loader,
                    >,
                >()
                .ok_or_else(|| {
                    ::nest_rs_resource::graphql::async_graphql::Error::new(::std::format!(
                        "relation `{}` is exposed but its dataloader `{}` is not seeded — the module providing it is not imported by (or reachable from) this app",
                        ::core::stringify!(#name),
                        ::core::any::type_name::<
                            ::nest_rs_resource::graphql::async_graphql::dataloader::DataLoader<
                                <#target as #related>::Loader,
                            >,
                        >(),
                    ))
                })?;
            // An unparsable cursor pages from the start rather than erroring —
            // the same contract `nest_rs_seaorm::PageParams::after_uuid` states
            // for the HTTP twin, so one malformed `after` cannot mean two things
            // depending on which transport carried it.
            let __after = ::core::option::Option::and_then(
                ::core::option::Option::as_deref(&after),
                |__c| ::core::result::Result::ok(
                    ::nest_rs_resource::uuid::Uuid::parse_str(__c),
                ),
            );
            let __page = __loader
                .load_one(::nest_rs_resource::RelationKey {
                    parent: #key_expr,
                    first: ::nest_rs_seaorm::clamp_page_size(::core::option::Option::unwrap_or(
                        first,
                        ::nest_rs_seaorm::DEFAULT_PAGE_SIZE,
                    )),
                    after: __after,
                })
                .await?
                .unwrap_or_default();
            ::core::result::Result::Ok(
                ::nest_rs_resource::RelationPage::into_connection(
                    __page,
                    ::core::option::Option::is_some(&__after),
                ),
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
            ::nest_rs_resource::uuid::Uuid::parse_str(&self.#ident)
                .map_err(|__e| ::nest_rs_resource::graphql::async_graphql::Error::new(__e.to_string()))?
        }
    } else {
        quote! { ::core::clone::Clone::clone(&self.#ident) }
    }
}
