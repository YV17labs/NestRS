//! Auto-generated bridges for entities: a PK loader on the entity's service
//! (so other entities can resolve `belongs_to` references without each one
//! re-declaring the loader), trait impls connecting the entity to its loader,
//! the wire DTO, and `#[ComplexObject]` field resolvers on the wire DTO for
//! every non-skip relation.
//!
//! Emission lives at the entity's call site (e.g. `users/entity.rs`); paths
//! resolve relative to that scope. Absolute paths are used for framework
//! crates so the user does not need to `use` them in `entity.rs`.
//!
//! Phase 1 — `belongs_to`: emits one `#[ComplexObject]` field per non-skip
//! `HasOne` plus the PK loader on the service.
//!
//! Phase 2 — `has_many`: emits one `#[ComplexObject]` field per non-skip
//! `HasMany`. The FK-side dataloader (`by_<fk_col>`) and the matching
//! `RelatedTo<Parent>` impl are emitted by the **FK-owning** entity (the side
//! that declares `belongs_to`), keeping every emission local to one module.

use nest_rs_codegen::pascal_case;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::attr::{RelationKind, ResourceField, ResourceModel, is_uuid};

pub fn emit(model: &ResourceModel) -> syn::Result<TokenStream2> {
    let Some(service) = model.service.clone() else {
        if model.has_auto_relations() {
            return Err(syn::Error::new_spanned(
                &model.source_ident,
                "this entity declares a non-skip relation but `#[expose(... service = …)]` is missing — add the service path so the macro can emit its PK dataloader and PkLoadable impl",
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
            "auto-relations on composite primary keys are not supported yet — write a hand-rolled `#[dataloader]` on the service and `#[expose(skip)]` the relation fields",
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

/// `#[dataloader] impl <Service> { async fn by_id(&self, keys: &[Pk]) -> ... }`.
/// Read-scoped via the ambient `Ability` — every call goes through `Repo`.
fn emit_pk_loader(model: &ResourceModel, service: &syn::Path, pk: &ResourceField) -> TokenStream2 {
    let pk_ident = &pk.ident;
    let pk_ty = &pk.ty;
    let pk_col = pascal_case(pk_ident);
    let wire = &model.output_ident;
    let target_label = format!("loading {} by id", wire);

    quote! {
        #[::nest_rs_graphql::dataloader]
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
                ::tracing::debug!(
                    target: "nest_rs::loader",
                    count = __keys.len(),
                    #target_label,
                );
                let __conn = ::nest_rs_seaorm::Repo::<Entity>::conn()?;
                let __rows = ::nest_rs_seaorm::Repo::<Entity>::scoped(
                    ::nest_rs_authz::Action::Read,
                )
                    .filter(
                        <Column as ::sea_orm::ColumnTrait>::is_in(
                            &Column::#pk_col,
                            __keys.iter().cloned(),
                        ),
                    )
                    .all(&__conn)
                    .await?;
                ::core::result::Result::Ok(
                    __rows
                        .into_iter()
                        .map(|__row| (__row.#pk_ident, <#wire as ::core::convert::From<&Model>>::from(&__row)))
                        .collect(),
                )
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

/// FK-side emission. For each non-skip `belongs_to` (the FK-owning side knows
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
    // even if the FK loaders were both registered. Refuse it at parse time.
    let mut seen_targets: Vec<(String, &Ident)> = Vec::new();
    for field in &model.fields {
        if field.skip {
            continue;
        }
        let Some(RelationKind::BelongsTo { from, target, .. }) = &field.relation else {
            continue;
        };
        let target_key = quote!(#target).to_string();
        if let Some((_, prev)) = seen_targets.iter().find(|(k, _)| k == &target_key) {
            return Err(syn::Error::new_spanned(
                &field.ident,
                format!(
                    "two `belongs_to` relations targeting the same parent are not supported (clashes with `{}`); mark one `#[expose(skip)]` and write a hand-rolled `#[field_resolver]`",
                    prev,
                ),
            ));
        }
        seen_targets.push((target_key, &field.ident));

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
        let loader_ident = format_ident!(
            "{}By{}",
            last_segment_ident(service),
            fk_col_pascal,
        );
        let wire = &model.output_ident;
        let target_label = format!("loading {} by {}", wire, from);

        blocks.push(quote! {
            #[::nest_rs_graphql::dataloader]
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
                    ::tracing::debug!(
                        target: "nest_rs::loader",
                        count = __keys.len(),
                        #target_label,
                    );
                    let __conn = ::nest_rs_seaorm::Repo::<Entity>::conn()?;
                    let __rows = ::nest_rs_seaorm::Repo::<Entity>::scoped(
                        ::nest_rs_authz::Action::Read,
                    )
                        .filter(
                            <Column as ::sea_orm::ColumnTrait>::is_in(
                                &Column::#fk_col_pascal,
                                __keys.iter().cloned(),
                            ),
                        )
                        .all(&__conn)
                        .await?;
                    let mut __map: ::std::collections::HashMap<#fk_ty, ::std::vec::Vec<#wire>> =
                        __keys
                            .iter()
                            .map(|__k| (::core::clone::Clone::clone(__k), ::std::vec::Vec::new()))
                            .collect();
                    for __row in __rows {
                        if let ::core::option::Option::Some(__bucket) = __map.get_mut(&__row.#from) {
                            __bucket.push(<#wire as ::core::convert::From<&Model>>::from(&__row));
                        }
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

/// `#[ComplexObject] impl <Wire> { … }` — one method per non-skip relation.
/// `BelongsTo` → `Option<TargetWire>` via `PkLoadable`. `HasMany` →
/// `Vec<TargetWire>` via `RelatedTo<Self::Entity>`.
fn emit_field_resolvers(model: &ResourceModel, pk: &ResourceField) -> syn::Result<TokenStream2> {
    let mut methods = Vec::new();
    for field in &model.fields {
        if field.skip {
            continue;
        }
        let Some(kind) = &field.relation else {
            continue;
        };
        match kind {
            RelationKind::BelongsTo { from, target, .. } => {
                methods.push(emit_belongs_to_method(model, &field.ident, from, target)?);
            }
            RelationKind::HasMany { target, .. } => {
                methods.push(emit_has_many_method(&field.ident, target, pk)?);
            }
        }
    }
    if methods.is_empty() {
        return Ok(TokenStream2::new());
    }
    let wire = &model.output_ident;
    Ok(quote! {
        #[::nest_rs_graphql::async_graphql::ComplexObject]
        impl #wire {
            #(#methods)*
        }
    })
}

/// One BelongsTo field resolver: load the parent's FK column via the target
/// entity's PK loader, returning its wire DTO.
fn emit_belongs_to_method(
    model: &ResourceModel,
    field: &Ident,
    fk: &Ident,
    target: &syn::Path,
) -> syn::Result<TokenStream2> {
    let fk_field = model.fields.iter().find(|f| &f.ident == fk).ok_or_else(|| {
        syn::Error::new_spanned(
            field,
            format!(
                "`belongs_to` declares `from = \"{}\"` but no column with that name is exposed on this entity",
                fk,
            ),
        )
    })?;

    let key_expr = wire_key_expr(&fk_field.ty, fk);

    Ok(quote! {
        async fn #field(
            &self,
            __ctx: &::nest_rs_graphql::async_graphql::Context<'_>,
        ) -> ::nest_rs_graphql::async_graphql::Result<
            ::core::option::Option<<#target as ::nest_rs_resource::PkLoadable>::Wire>,
        > {
            let __loader = __ctx.data_unchecked::<
                ::nest_rs_graphql::async_graphql::dataloader::DataLoader<
                    <#target as ::nest_rs_resource::PkLoadable>::Loader,
                >,
            >();
            let __key = #key_expr;
            ::core::result::Result::Ok(__loader.load_one(__key).await?)
        }
    })
}

/// One HasMany field resolver: load the children of `self` via the target's
/// `RelatedTo<Self::Entity>::Loader`, keyed on `self`'s PK. The target's macro
/// is responsible for declaring the `RelatedTo` impl from its own `belongs_to`.
fn emit_has_many_method(
    field: &Ident,
    target: &syn::Path,
    pk: &ResourceField,
) -> syn::Result<TokenStream2> {
    let key_expr = wire_key_expr(&pk.ty, &pk.ident);

    Ok(quote! {
        async fn #field(
            &self,
            __ctx: &::nest_rs_graphql::async_graphql::Context<'_>,
        ) -> ::nest_rs_graphql::async_graphql::Result<
            ::std::vec::Vec<<#target as ::nest_rs_resource::RelatedTo<Entity>>::Wire>,
        > {
            let __loader = __ctx.data_unchecked::<
                ::nest_rs_graphql::async_graphql::dataloader::DataLoader<
                    <#target as ::nest_rs_resource::RelatedTo<Entity>>::Loader,
                >,
            >();
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
                .map_err(|__e| ::nest_rs_graphql::async_graphql::Error::new(__e.to_string()))?
        }
    } else {
        quote! { ::core::clone::Clone::clone(&self.#ident) }
    }
}

/// Last segment of a `syn::Path`. `syn::Path::parse` guarantees at least one
/// segment, so the index is infallible — kept as an inlined ident lookup.
fn last_segment_ident(path: &syn::Path) -> &Ident {
    &path.segments.last().expect("syn::Path has ≥ 1 segment").ident
}
