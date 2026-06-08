//! Parse `#[expose(...)]` into a [`ResourceModel`] and strip the per-field
//! annotations so the ORM macros see a clean entity.

use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;
use syn::parse::Parse;
use syn::{
    Fields, GenericArgument, Ident, ItemStruct, LitStr, Path, PathArguments, Token, Type, TypePath,
};

/// SeaORM marker on a relation field: `HasOne<T>` ⇔ `belongs_to`,
/// `HasMany<T>` ⇔ `has_many`. Kept typed (not stringly) so a rename or typo
/// on either side fails at compile rather than as a silent scalar fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cardinality {
    One,
    Many,
}

/// What kind of SeaORM association the field declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RelationKind {
    /// Owner of the foreign key — `#[sea_orm(belongs_to, from = …, to = …)]`
    /// paired with `HasOne<T>`. Resolves to one target via its PK loader.
    BelongsTo {
        /// FK column on the current entity (e.g. `org_id`).
        from: Ident,
        /// `crate::orgs::Entity` (the path written between `HasOne<…>`).
        target: Path,
    },
    /// Inverse side — `#[sea_orm(has_many)]` on a `HasMany<T>`. The target's
    /// own `belongs_to` macro is responsible for emitting the FK loader; this
    /// side only consumes `RelatedTo<Self::Entity>::Loader`.
    HasMany {
        /// `crate::users::Entity`.
        target: Path,
    },
}

pub(crate) struct ResourceField {
    pub ident: Ident,
    pub ty: Type,
    /// Excluded from the GraphQL output type AND, when this is a relation,
    /// from auto-generated field-resolver emission.
    pub skip: bool,
    pub in_create: bool,
    pub in_update: bool,
    /// The `#[sea_orm(primary_key)]` column — seeded with UUID v7 by the
    /// generated `create` when its type is `Uuid`.
    pub is_pk: bool,
    /// Re-emitted verbatim as `#[validate(...)]` on the input field.
    pub validate: Vec<TokenStream2>,
    /// Detected `HasOne<T>` / `HasMany<T>` association. Drives auto-generated
    /// field resolvers + loader trait impls. Scalar columns leave this `None`.
    pub relation: Option<RelationKind>,
}

impl ResourceField {
    /// True iff the field belongs in the output struct as a plain column. A
    /// relation never does — it is materialised by a `#[ComplexObject]` field
    /// resolver (or skipped entirely).
    pub fn in_output_struct(&self) -> bool {
        !self.skip && self.relation.is_none()
    }
}

pub(crate) struct ResourceModel {
    pub source_ident: Ident,
    pub output_ident: Ident,
    pub create_input_ident: Ident,
    pub update_input_ident: Ident,
    pub page_ident: Ident,
    pub fields: Vec<ResourceField>,
    /// Path to the entity's service, used as the receiver of auto-generated
    /// `#[dataloader]` impls. Required when any non-skip relation is present.
    pub service: Option<Path>,
    /// Emit `#[graphql(complex)]` on the output. Set explicitly via
    /// `complex` or implicitly when any non-skip relation calls for a
    /// `#[ComplexObject]`.
    pub complex: bool,
    pub paginate: bool,
}

impl ResourceModel {
    /// True iff at least one non-skip relation needs a `#[ComplexObject]`.
    pub fn has_auto_relations(&self) -> bool {
        self.fields.iter().any(|f| !f.skip && f.relation.is_some())
    }
}

pub(crate) fn parse(args: TokenStream2, item: &mut ItemStruct) -> syn::Result<ResourceModel> {
    let mut name: Option<String> = None;
    let mut service: Option<Path> = None;
    let mut complex = false;
    let mut paginate = false;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("service") {
            service = Some(meta.value()?.parse::<Path>()?);
            Ok(())
        } else if meta.path.is_ident("complex") {
            complex = true;
            Ok(())
        } else if meta.path.is_ident("paginate") {
            paginate = true;
            Ok(())
        } else {
            Err(meta.error(
                "unknown #[expose(...)] option (expected `name = \"...\"`, `service = …`, `complex`, or `paginate`)",
            ))
        }
    });
    syn::parse::Parser::parse2(parser, args)?;

    let name = name.ok_or_else(|| {
        syn::Error::new_spanned(
            &item.ident,
            "#[expose(name = \"...\")] is required (the GraphQL type / input base name)",
        )
    })?;
    let name_ident = format_ident!("{}", name);
    let source_ident = item.ident.clone();

    let Fields::Named(named) = &mut item.fields else {
        return Err(syn::Error::new_spanned(
            &item.fields,
            "#[expose] requires a struct with named fields (a SeaORM entity `Model`)",
        ));
    };

    let mut fields = Vec::new();
    for field in &mut named.named {
        let ident = field.ident.clone().expect("named field has an ident");
        let ty = field.ty.clone();
        let mut skip = false;
        let mut in_create = false;
        let mut in_update = false;
        let mut validate = Vec::new();

        // Pull PK + relation column info out of the `#[sea_orm(...)]` attrs in
        // the same pass. The attrs stay on the field so SeaORM still owns them
        // — we only read.
        let mut is_pk = false;
        let mut is_belongs_to = false;
        let mut is_has_many = false;
        let mut from_col: Option<String> = None;
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("sea_orm")) {
            // Surface a sea_orm-side parse failure — silently swallowing it
            // (the previous `let _ = ...`) hid malformed `from = some_expr`
            // shapes behind a downstream 'missing from' diagnostic.
            attr.parse_nested_meta(|m| {
                if m.path.is_ident("primary_key") {
                    is_pk = true;
                } else if m.path.is_ident("belongs_to") {
                    is_belongs_to = true;
                    // Legacy `belongs_to = "Path"` form: accept and ignore the
                    // value. The flat form (`#[sea_orm(belongs_to, …)]`) is the
                    // canonical one in this repo.
                    if m.input.peek(Token![=]) {
                        let _: syn::Expr = m.value()?.parse()?;
                    }
                } else if m.path.is_ident("has_many") {
                    is_has_many = true;
                    if m.input.peek(Token![=]) {
                        let _: syn::Expr = m.value()?.parse()?;
                    }
                } else if m.path.is_ident("from") {
                    from_col = Some(m.value()?.parse::<LitStr>()?.value());
                } else if m.input.peek(Token![=]) {
                    // Any other key-value pair — consume so the meta parser
                    // can advance past it without erroring.
                    let _: syn::Expr = m.value()?.parse()?;
                }
                Ok(())
            })?;
        }

        for attr in field.attrs.iter().filter(|a| a.path().is_ident("expose")) {
            attr.parse_nested_meta(|m| {
                if m.path.is_ident("skip") {
                    skip = true;
                } else if m.path.is_ident("input") {
                    let content;
                    syn::parenthesized!(content in m.input);
                    let kinds = content.parse_terminated(Ident::parse, Token![,])?;
                    for k in kinds {
                        if k == "create" {
                            in_create = true;
                        } else if k == "update" {
                            in_update = true;
                        } else {
                            return Err(syn::Error::new(k.span(), "expected `create` or `update`"));
                        }
                    }
                } else if m.path.is_ident("validate") {
                    let content;
                    syn::parenthesized!(content in m.input);
                    validate.push(content.parse()?);
                } else {
                    return Err(m.error(
                        "unknown #[expose(...)] field option (expected `skip`, `input(...)`, or `validate(...)`)",
                    ));
                }
                Ok(())
            })?;
        }

        if skip && (in_create || in_update) {
            return Err(syn::Error::new_spanned(
                &field.ident,
                "a `skip` field cannot also be an `input`",
            ));
        }

        field.attrs.retain(|a| !a.path().is_ident("expose"));

        // Type-driven relation detection. `HasOne<T>` paired with `belongs_to`
        // ⇒ BelongsTo; `HasMany<T>` paired with `has_many` ⇒ HasMany. A type
        // marker without its matching sea_orm marker is a user mistake worth
        // surfacing — silently treating it as a scalar drops the field into
        // the `SimpleObject` derive where it explodes with a cryptic
        // 'HasOne does not impl OutputType' span on the macro expansion.
        let card = relation_cardinality(&ty);
        let relation = match (card, is_belongs_to, is_has_many) {
            (Some((Cardinality::One, target)), true, _) => {
                let from = from_col.ok_or_else(|| {
                    syn::Error::new_spanned(
                        &field.ident,
                        "`belongs_to` relation needs `#[sea_orm(from = \"...\")]`",
                    )
                })?;
                Some(RelationKind::BelongsTo {
                    from: format_ident!("{}", from),
                    target,
                })
            }
            (Some((Cardinality::Many, target)), _, true) => Some(RelationKind::HasMany { target }),
            (Some((Cardinality::One, _)), false, _) => {
                return Err(syn::Error::new_spanned(
                    &field.ident,
                    "`HasOne<T>` field is missing its `#[sea_orm(belongs_to, from = \"...\", to = \"...\")]` marker",
                ));
            }
            (Some((Cardinality::Many, _)), _, false) => {
                return Err(syn::Error::new_spanned(
                    &field.ident,
                    "`HasMany<T>` field is missing its `#[sea_orm(has_many)]` marker",
                ));
            }
            _ => None,
        };

        fields.push(ResourceField {
            ident,
            ty,
            skip,
            in_create,
            in_update,
            is_pk,
            validate,
            relation,
        });
    }

    Ok(ResourceModel {
        source_ident,
        output_ident: name_ident.clone(),
        create_input_ident: format_ident!("Create{}Input", name_ident),
        update_input_ident: format_ident!("Update{}Input", name_ident),
        page_ident: format_ident!("{}Page", name_ident),
        fields,
        service,
        complex,
        paginate,
    })
}

/// Match `HasOne<T>` / `HasMany<T>` on the last path segment. Returns the
/// cardinality and the inner target path.
fn relation_cardinality(ty: &Type) -> Option<(Cardinality, Path)> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return None;
    };
    let last = path.segments.last()?;
    let card = match last.ident.to_string().as_str() {
        "HasOne" => Cardinality::One,
        "HasMany" => Cardinality::Many,
        _ => return None,
    };
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let GenericArgument::Type(Type::Path(target)) = args.args.first()? else {
        return None;
    };
    Some((card, target.path.clone()))
}

/// `true` when the type's last path segment is `Uuid` (rendered as `String` in
/// the GraphQL output). Purely syntactic: `Option<Uuid>` and aliases pass
/// through with their native type.
pub(crate) fn is_uuid(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Uuid"))
}
