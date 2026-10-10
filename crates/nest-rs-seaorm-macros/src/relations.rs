//! Auto-generated bridges for entities: a PK loader on the entity's service,
//! trait impls connecting the entity to its loader and wire DTO, and
//! `#[ComplexObject]` field resolvers for every exposed (`#[expose]`) relation.
//!
//! The FK-side dataloader (`by_<fk_col>`) and its `RelatedTo<Parent, Via>` impl
//! are emitted by the entity declaring `belongs_to`, never by the parent.

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
/// `20` and `1..=100` mirror `nest_rs_seaorm::DEFAULT_PAGE_SIZE` and
/// `clamp_page_size`: a string, so no path in it is re-rooted. The clamp is
/// load-bearing — `first` arrives unclamped as any `u64`, and the product would
/// overflow (a debug panic, a release wrap under `max_complexity`).
pub(crate) const DEFAULT_HAS_MANY_COMPLEXITY: &str =
    "first.unwrap_or(20).clamp(1, 100) as usize * child_complexity";

pub(crate) fn emit(model: &ResourceModel) -> syn::Result<TokenStream2> {
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
        quote! { ::nest_rs_seaorm::sea_orm::Condition::all() }
    }
}

/// `#[dataloader] impl <Service> { async fn by_id(&self, keys: &[Pk]) -> ... }`.
fn emit_pk_loader(model: &ResourceModel, service: &syn::Path, pk: &ResourceField) -> TokenStream2 {
    let pk_ident = &pk.ident;
    let pk_ty = &pk.ty;
    let pk_col = pascal_case(pk_ident);
    let wire = &model.output_ident;
    let target_label = format!("loading {} by id", wire);
    let live = live_rows_filter(model);

    quote! {
        #[::nest_rs_seaorm::__private::dataloader]
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
                ::nest_rs_seaorm::__private::tracing::debug!(
                    target: ::nest_rs_seaorm::target::LOADER,
                    count = __keys.len(),
                    #target_label,
                );
                let __conn = ::nest_rs_seaorm::Repo::<Entity>::conn()?;
                let __rows = ::nest_rs_seaorm::Repo::<Entity>::scoped(
                    ::nest_rs_authz::Action::Read,
                )
                    #live
                    .filter(
                        <Column as ::nest_rs_seaorm::sea_orm::ColumnTrait>::is_in(
                            &Column::#pk_col,
                            __keys.iter().cloned(),
                        ),
                    )
                    .all(&__conn)
                    .await?;
                // `scoped(Read)` filters rows only; columns are masked here.
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
        impl ::nest_rs_seaorm::graphql::PkLoadable for Entity {
            type Loader = #loader;
            type Wire = #wire;
        }
    }
}

/// The scalar column a `belongs_to` names in `from = "…"`, or the refusal both
/// emission sites raise when the entity has no such column. Exposure is not
/// required: a loader keys on the entity's `Column`.
fn fk_column<'a>(model: &'a ResourceModel, fk: &Ident) -> syn::Result<&'a ResourceField> {
    model.fields.iter().find(|f| &f.ident == fk).ok_or_else(|| {
        syn::Error::new_spanned(
            fk,
            format!(
                "{}: \"{fk}\" is not a column of this entity — `#[expose]` reads it as the field \
                 holding this relation's foreign key",
                nest_rs_codegen::site("sea_orm", Some("from")),
            ),
        )
    })
}

/// The same column, additionally required to carry `#[expose]`: the field
/// resolver reads the key off the wire object, which holds exposed columns only.
fn exposed_fk_column<'a>(
    model: &'a ResourceModel,
    relation: &Ident,
    fk: &Ident,
) -> syn::Result<&'a ResourceField> {
    let column = fk_column(model, fk)?;
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

/// FK-side emission: for each exposed `belongs_to`, a `by_<fk_col>` batched
/// loader on the service plus its `RelatedTo<TargetEntity>` impls.
fn emit_fk_loaders(
    model: &ResourceModel,
    service: &syn::Path,
    pk: &ResourceField,
) -> syn::Result<TokenStream2> {
    let mut blocks = Vec::new();
    // A parent named twice gets no `SoleForeignKey` impl: two would be E0119.
    // Keyed by module path, never the last segment — every SeaORM entity is `Entity`.
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

        let fk_ty = &fk_column(model, from)?.ty;
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
        let sole_impl = sole.then(|| {
            quote! {
                impl ::nest_rs_seaorm::graphql::RelatedTo<#target> for Entity {
                    type Loader = #loader_ident;
                    type Wire = #wire;
                }
            }
        });
        let target_label = format!("paging {} by {}", wire, from);
        let live = live_rows_condition(model);
        let pk_ident = &pk.ident;

        blocks.push(quote! {
            #[::nest_rs_seaorm::__private::dataloader]
            impl #service {
                async fn #method_name(
                    &self,
                    __keys: &[::nest_rs_seaorm::graphql::RelationKey<#fk_ty>],
                ) -> ::core::result::Result<
                    ::std::collections::HashMap<
                        ::nest_rs_seaorm::graphql::RelationKey<#fk_ty>,
                        ::nest_rs_seaorm::graphql::RelationPage<#wire>,
                    >,
                    ::nest_rs_seaorm::ServiceError,
                > {
                    let mut __out: ::std::collections::HashMap<
                        ::nest_rs_seaorm::graphql::RelationKey<#fk_ty>,
                        ::nest_rs_seaorm::graphql::RelationPage<#wire>,
                    > = ::std::collections::HashMap::with_capacity(__keys.len());
                    if __keys.is_empty() {
                        return ::core::result::Result::Ok(__out);
                    }
                    ::nest_rs_seaorm::__private::tracing::debug!(
                        target: ::nest_rs_seaorm::target::LOADER,
                        count = __keys.len(),
                        #target_label,
                    );
                    // Two aliases of one relation may ask for different windows.
                    let mut __windows: ::std::vec::Vec<(
                        u64,
                        ::core::option::Option<::nest_rs_seaorm::__private::uuid::Uuid>,
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
                        // Ranked per parent: a shared `LIMIT n` would starve
                        // later parents into an empty page.
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
                                // `scoped(Read)` filters rows only; columns are masked here.
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
                                ::nest_rs_seaorm::graphql::RelationKey {
                                    parent: __parent,
                                    first: __first,
                                    after: __after,
                                },
                                ::nest_rs_seaorm::graphql::RelationPage {
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

            impl ::nest_rs_seaorm::graphql::RelatedTo<#target, #via_ident> for Entity {
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
fn via_marker_ident(column: &Ident) -> Ident {
    format_ident!("By{}", pascal_case(column), span = column.span())
}

/// `RelatedTo<Entity, Via>` for one `HasMany`, as the trait-path half of a
/// `<Child as …>::Loader` projection.
///
/// The marker is reached beside the child entity as written
/// (`crate::posts::ByAuthorId`), so a module re-exporting the entity must
/// re-export the marker too.
fn related_to_path(target: &syn::Path, via: Option<&syn::LitStr>) -> syn::Result<TokenStream2> {
    let Some(via) = via else {
        return Ok(quote! { ::nest_rs_seaorm::graphql::RelatedTo<Entity> });
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
    Ok(quote! { ::nest_rs_seaorm::graphql::RelatedTo<Entity, #marker> })
}

/// The parent entity path, normalized for counting: a leading `crate::`/`self::`
/// is stripped so `crate::orgs::Entity` and `orgs::Entity` count as one parent.
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
/// entity's PK loader, returning its wire DTO.
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
    let root = graphql_root();

    Ok(quote! {
        #complexity
        async fn #name(
            &self,
            __ctx: &#root::Context<'_>,
        ) -> #root::Result<
            ::core::option::Option<<#target as ::nest_rs_seaorm::graphql::PkLoadable>::Wire>,
        > {
            // Never `data_unchecked`: a loader whose owner module this app does
            // not import would panic at request time.
            let __loader = __ctx
                .data_opt::<
                    #root::dataloader::DataLoader<
                        <#target as ::nest_rs_seaorm::graphql::PkLoadable>::Loader,
                    >,
                >()
                .ok_or_else(|| {
                    #root::Error::new(::std::format!(
                        "relation `{}` is exposed but its dataloader `{}` is not seeded — the module providing it is not imported by (or reachable from) this app",
                        ::core::stringify!(#name),
                        ::core::any::type_name::<
                            #root::dataloader::DataLoader<
                                <#target as ::nest_rs_seaorm::graphql::PkLoadable>::Loader,
                            >,
                        >(),
                    ))
                })?;
            let __key = #key_expr;
            ::core::result::Result::Ok(__loader.load_one(__key).await?)
        }
    })
}

/// One HasMany field resolver: one page of the children of `self`, as a Relay
/// `Connection` cursored by the child's primary key, through the target's
/// `RelatedTo<Self::Entity, Via>::Loader`.
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
    let root = graphql_root();

    Ok(quote! {
        #complexity
        async fn #name(
            &self,
            __ctx: &#root::Context<'_>,
            first: ::core::option::Option<u64>,
            after: ::core::option::Option<::std::string::String>,
        ) -> #root::Result<
            #root::connection::Connection<
                ::std::string::String,
                <#target as #related>::Wire,
            >,
        > {
            // Never `data_unchecked`: a loader whose owner module this app does
            // not import would panic at request time.
            let __loader = __ctx
                .data_opt::<
                    #root::dataloader::DataLoader<
                        <#target as #related>::Loader,
                    >,
                >()
                .ok_or_else(|| {
                    #root::Error::new(::std::format!(
                        "relation `{}` is exposed but its dataloader `{}` is not seeded — the module providing it is not imported by (or reachable from) this app",
                        ::core::stringify!(#name),
                        ::core::any::type_name::<
                            #root::dataloader::DataLoader<
                                <#target as #related>::Loader,
                            >,
                        >(),
                    ))
                })?;
            // An unparsable cursor pages from the start, as
            // `nest_rs_seaorm::PageParams::after_uuid` does over HTTP.
            let __after = ::core::option::Option::and_then(
                ::core::option::Option::as_deref(&after),
                |__c| ::core::result::Result::ok(
                    ::nest_rs_seaorm::__private::uuid::Uuid::parse_str(__c),
                ),
            );
            let __page = __loader
                .load_one(::nest_rs_seaorm::graphql::RelationKey {
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
                ::nest_rs_seaorm::graphql::RelationPage::into_connection(
                    __page,
                    ::core::option::Option::is_some(&__after),
                ),
            )
        }
    })
}

/// The wire representation of a column → key the dataloader expects: a `Uuid`
/// is a `String` on the wire, so it is parsed back.
fn wire_key_expr(ty: &Type, ident: &Ident) -> TokenStream2 {
    if is_uuid(ty) {
        let root = graphql_root();
        quote! {
            ::nest_rs_seaorm::__private::uuid::Uuid::parse_str(&self.#ident)
                .map_err(|__e| #root::Error::new(__e.to_string()))?
        }
    } else {
        quote! { ::core::clone::Clone::clone(&self.#ident) }
    }
}
