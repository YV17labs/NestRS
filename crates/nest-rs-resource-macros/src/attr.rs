//! Parse `#[expose(...)]` into a [`ResourceModel`] and strip the per-field
//! annotations so the ORM macros see a clean entity.

use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;
use syn::parse::Parse;
use syn::{Fields, Ident, ItemStruct, LitStr, Token, Type};

pub struct ResourceField {
    pub ident: Ident,
    pub ty: Type,
    /// Excluded from the GraphQL output type.
    pub skip: bool,
    pub in_create: bool,
    pub in_update: bool,
    /// The `#[sea_orm(primary_key)]` column — seeded with UUID v7 by the
    /// generated `create` when its type is `Uuid`.
    pub is_pk: bool,
    /// Re-emitted verbatim as `#[validate(...)]` on the input field.
    pub validate: Vec<TokenStream2>,
}

impl ResourceField {
    pub fn in_output(&self) -> bool {
        !self.skip
    }
}

pub struct ResourceModel {
    pub source_ident: Ident,
    pub output_ident: Ident,
    pub create_input_ident: Ident,
    pub update_input_ident: Ident,
    pub page_ident: Ident,
    pub fields: Vec<ResourceField>,
    /// Emit `#[graphql(complex)]` so a bespoke `#[field]` resolver can attach.
    pub complex: bool,
    pub paginate: bool,
}

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

        // Detect the PK from the untouched `#[sea_orm(...)]` attrs so `create`
        // can seed a UUID v7. Name-value pairs (e.g. `from = "col"`) must be
        // consumed for the meta parser to advance; errors are ignored — an
        // unreadable column simply is not flagged as PK.
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
/// the GraphQL output). Purely syntactic: `Option<Uuid>` and aliases pass
/// through with their native type.
pub fn is_uuid(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Uuid"))
}
