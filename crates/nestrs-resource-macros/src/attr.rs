//! Parse `#[expose(...)]` on a SeaORM entity into a [`ResourceModel`] the
//! emitters consume, and strip the per-field `#[expose(...)]` annotations from
//! the struct so the ORM macros (`#[sea_orm::model]` / `DeriveEntityModel`) see
//! a clean entity. The grammar is deliberately small and NestJS-shaped: it
//! declares only how the *type* is exposed (which fields, which inputs) — never
//! routes or guards, which belong on controllers/resolvers.

use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;
use syn::parse::Parse;
use syn::{Fields, Ident, ItemStruct, LitStr, Token, Type};

/// One column of the entity with its resolved exposure flags.
pub struct ResourceField {
    pub ident: Ident,
    pub ty: Type,
    /// Excluded from the GraphQL output type (e.g. a server-side scope column).
    pub skip: bool,
    pub in_create: bool,
    pub in_update: bool,
    /// The `#[sea_orm(primary_key)]` column — the generated `create` conversion
    /// seeds it with a fresh UUID v7 when its type is `Uuid`.
    pub is_pk: bool,
    /// Raw `validate(...)` bodies, re-emitted verbatim on the input field.
    pub validate: Vec<TokenStream2>,
}

impl ResourceField {
    /// Present in the GraphQL output object?
    pub fn in_output(&self) -> bool {
        !self.skip
    }
}

/// The resolved exposure: the generated item names plus the per-field flags.
pub struct ResourceModel {
    /// The entity struct the `From` conversion reads from (SeaORM's `Model`).
    pub source_ident: Ident,
    pub output_ident: Ident,
    pub create_input_ident: Ident,
    pub update_input_ident: Ident,
    /// `<Name>Page` — the pagination envelope emitted when `paginate` is set.
    pub page_ident: Ident,
    pub fields: Vec<ResourceField>,
    /// Emit `#[graphql(complex)]` so a bespoke `#[field]` resolver can attach.
    pub complex: bool,
    /// Emit a `<Name>Page` envelope (`#[expose(paginate)]`) for list endpoints.
    pub paginate: bool,
}

/// Parse the `#[expose(...)]` attribute arguments and field annotations,
/// removing the field annotations from `item` in place.
pub fn parse(args: TokenStream2, item: &mut ItemStruct) -> syn::Result<ResourceModel> {
    let mut name: Option<String> = None;
    let mut complex = false;
    let mut paginate = false;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("complex") {
            complex = true;
            Ok(())
        } else if meta.path.is_ident("paginate") {
            paginate = true;
            Ok(())
        } else {
            Err(meta.error(
                "unknown #[expose(...)] option (expected `name = \"...\"`, `complex`, or `paginate`)",
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

        // Detect the primary key from the (untouched) `#[sea_orm(...)]` attrs, so
        // the generated `create` conversion can seed a UUID-v7 id. Each name-value
        // meta's value is consumed so SeaORM grammar like `from = "col"` parses;
        // errors are ignored — a column we cannot read just is not flagged as PK.
        let mut is_pk = false;
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("sea_orm")) {
            let _ = attr.parse_nested_meta(|m| {
                if m.path.is_ident("primary_key") {
                    is_pk = true;
                }
                if m.input.peek(Token![=]) {
                    let _: syn::Expr = m.value()?.parse()?;
                }
                Ok(())
            });
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

        // Strip our annotations so the ORM macros see a clean entity.
        field.attrs.retain(|a| !a.path().is_ident("expose"));

        fields.push(ResourceField {
            ident,
            ty,
            skip,
            in_create,
            in_update,
            is_pk,
            validate,
        });
    }

    Ok(ResourceModel {
        source_ident,
        output_ident: name_ident.clone(),
        create_input_ident: format_ident!("Create{}Input", name_ident),
        update_input_ident: format_ident!("Update{}Input", name_ident),
        page_ident: format_ident!("{}Page", name_ident),
        fields,
        complex,
        paginate,
    })
}

/// `true` when the type's last path segment is `Uuid` (rendered as `String` in
/// the GraphQL output object, mirroring the hand-written DTOs). Purely
/// syntactic: `Option<Uuid>` and aliases of `Uuid` are *not* matched (their last
/// segment isn't `Uuid`), so they pass through with their native type.
pub fn is_uuid(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Uuid"))
}
