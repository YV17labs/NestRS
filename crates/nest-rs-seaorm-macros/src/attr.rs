//! Parse `#[expose(...)]` into a [`ResourceModel`] and strip the per-field
//! annotations so the ORM macros see a clean entity.

use nest_rs_codegen::{Arg, Grammar, ungrouped_expr};
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote};
use syn::parse::Parse;
use syn::{
    Expr, Fields, GenericArgument, Ident, ItemStruct, LitStr, Path, PathArguments, Token, Type,
    TypePath,
};

/// SeaORM marker on a relation field: `HasOne<T>` ⇔ `belongs_to`,
/// `HasMany<T>` ⇔ `has_many`.
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
    /// side only consumes `RelatedTo<Self::Entity, Via>::Loader`.
    HasMany {
        /// `crate::users::Entity`.
        target: Path,
        /// `#[expose(via = "author_id")]`; `None` ⇒ `SoleForeignKey`. Kept as the
        /// [`LitStr`](struct@LitStr) written so a bad column name reports at the string.
        via: Option<LitStr>,
    },
}

pub(crate) struct ResourceField {
    pub ident: Ident,
    pub ty: Type,
    /// Whether the field carries `#[expose]` in any form; without it the field
    /// is hidden from every transport.
    pub read: bool,
    pub in_create: bool,
    pub in_update: bool,
    /// The `#[sea_orm(primary_key)]` column — seeded with UUID v7 by the
    /// generated `create` when its type is `Uuid`.
    pub is_pk: bool,
    /// Re-emitted verbatim as `#[validate(...)]` on the input field.
    pub validate: Vec<TokenStream2>,
    /// Detected `HasOne<T>` / `HasMany<T>` association; `None` on a scalar column.
    pub relation: Option<RelationKind>,
    /// Override async-graphql's per-field complexity for the auto-emitted
    /// field resolver; `None` ⇒ a default per relation kind (`relations::emit`).
    pub complexity: Option<Expr>,
    /// `#[wire_default]` (`Some(None)`, the type's `Default`) or
    /// `#[wire_default(expr)]`; sound only when no `Ability` rule predicates on the column.
    pub wire_default: Option<Option<Expr>>,
}

impl ResourceField {
    pub(crate) fn in_output_struct(&self) -> bool {
        self.read && self.relation.is_none()
    }
}

pub(crate) fn complexity_attr(user: &Option<Expr>, default: Option<&str>) -> TokenStream2 {
    if let Some(expr) = user {
        return quote! { #[graphql(complexity = #expr)] };
    }
    if let Some(s) = default {
        let lit = LitStr::new(s, proc_macro2::Span::call_site());
        return quote! { #[graphql(complexity = #lit)] };
    }
    TokenStream2::new()
}

pub(crate) struct ResourceModel {
    pub source_ident: Ident,
    pub output_ident: Ident,
    pub create_ident: Ident,
    pub update_ident: Ident,
    pub fields: Vec<ResourceField>,
    /// Path to the entity's service, used as the receiver of auto-generated
    /// `#[dataloader]` impls. Required when any exposed relation is present.
    pub service: Option<Path>,
    /// Emit `#[graphql(complex)]` on the output: `complex`, or implied by an
    /// exposed relation.
    pub complex: bool,
    /// When set, emit GraphQL surface types (SimpleObject, loaders, relations).
    pub graphql: bool,
    /// Stamp `deleted_at` instead of hard-deleting; emit `SoftDeletable`.
    pub soft_delete: bool,
    /// Maintain `created_at` / `updated_at` via `ActiveModelBehavior::before_save`.
    pub timestamps: bool,
}

impl ResourceModel {
    pub(crate) fn has_auto_relations(&self) -> bool {
        self.fields.iter().any(|f| f.read && f.relation.is_some())
    }
}

pub(crate) fn parse(args: TokenStream2, item: &mut ItemStruct) -> syn::Result<ResourceModel> {
    let mut name: Option<String> = None;
    let mut service: Option<Path> = None;
    let mut complex = false;
    let mut graphql = false;
    let mut soft_delete = false;
    let mut timestamps = false;
    MODEL.parse2(args, |arg| {
        match arg.key() {
            "name" => name = Some(type_name(&arg.expr()?)?),
            "service" => {
                service = Some(arg.value()?.parse::<Path>().map_err(|stopped| {
                    syn::Error::new(
                        stopped.span(),
                        nest_rs_codegen::takes_value(
                            "expose",
                            Some("service"),
                            "the path of the entity's service, e.g. `service = UsersService`",
                        ),
                    )
                })?);
            }
            "complex" => complex = true,
            "graphql" => graphql = true,
            "soft_delete" => soft_delete = true,
            "timestamps" => timestamps = true,
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;

    let name = name.ok_or_else(|| {
        syn::Error::new_spanned(
            &item.ident,
            format!(
                "{} (the wire DTO and OpenAPI schema name)",
                nest_rs_codegen::missing_argument("expose", "name", "\"User\""),
            ),
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
        #[expect(
            clippy::expect_used,
            reason = "a compile-time invariant of the parse above; a panic in a proc macro is a compile error"
        )]
        let ident = field.ident.clone().expect("named field has an ident");
        let ty = field.ty.clone();
        let mut read = false;
        let mut in_create = false;
        let mut in_update = false;
        let mut validate = Vec::new();
        let mut complexity: Option<Expr> = None;
        let mut via: Option<LitStr> = None;
        let mut is_pk = false;
        let mut is_belongs_to = false;
        let mut is_has_many = false;
        let mut from_col: Option<Ident> = None;
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("sea_orm")) {
            attr.parse_nested_meta(|m| {
                if m.path.is_ident("primary_key") {
                    is_pk = true;
                } else if m.path.is_ident("belongs_to") {
                    is_belongs_to = true;
                    // Legacy `belongs_to = "Path"` form: accept and ignore the value.
                    if m.input.peek(Token![=]) {
                        let _: syn::Expr = m.value()?.parse()?;
                    }
                } else if m.path.is_ident("has_many") {
                    is_has_many = true;
                    if m.input.peek(Token![=]) {
                        let _: syn::Expr = m.value()?.parse()?;
                    }
                } else if m.path.is_ident("from") {
                    let written: Expr = m.value()?.parse()?;
                    from_col = Some(foreign_key(&written)?);
                } else if m.input.peek(Token![=]) {
                    // Consume any other value so the meta parser can advance.
                    let _: syn::Expr = m.value()?.parse()?;
                }
                Ok(())
            })?;
        }

        // Every `#[expose]` on the field is read as one list, so a key written
        // in two of them is a repeat the grammar refuses.
        let mut written = TokenStream2::new();
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("expose")) {
            read = true;
            if let syn::Meta::List(list) = &attr.meta
                && !list.tokens.is_empty()
            {
                if !written.is_empty() {
                    written.extend(quote!(,));
                }
                written.extend(list.tokens.clone());
            }
        }
        FIELD.parse2(written, |arg| {
            match arg.key() {
                "input" => {
                    for kind in listed(
                        &arg,
                        "a list of `create` and `update`, e.g. `input(create, update)`",
                    )? {
                        match ungrouped_expr(&kind) {
                            Expr::Path(p) if p.path.is_ident("create") => in_create = true,
                            Expr::Path(p) if p.path.is_ident("update") => in_update = true,
                            other => {
                                return Err(syn::Error::new_spanned(
                                    other,
                                    nest_rs_codegen::unknown_value(
                                        "expose",
                                        "input",
                                        &other.to_token_stream().to_string(),
                                        &["create", "update"],
                                    ),
                                ));
                            }
                        }
                    }
                }
                "validate" => {
                    let input = arg.input();
                    if !input.peek(syn::token::Paren) {
                        return Err(syn::Error::new(
                            arg.ident().span(),
                            nest_rs_codegen::takes_value(
                                "expose",
                                Some("validate"),
                                "a list of `validator` rules, e.g. `validate(length(min = 1))`",
                            ),
                        ));
                    }
                    let content;
                    syn::parenthesized!(content in input);
                    validate.push(content.parse()?);
                }
                // A literal int or an expression string async-graphql parses,
                // re-emitted verbatim; only a `HasMany` has `first`/`after` to name.
                "complexity" => complexity = Some(arg.expr()?),
                "via" => {
                    let lit = arg.str_lit("author_id")?;
                    if syn::parse_str::<Ident>(&lit.value()).is_err() {
                        return Err(syn::Error::new_spanned(
                            &lit,
                            format!(
                                "{}: {:?} is not a column name — it takes a snake_case column \
                                 of the child entity, e.g. `via = \"author_id\"`",
                                nest_rs_codegen::site("expose", Some("via")),
                                lit.value(),
                            ),
                        ));
                    }
                    via = Some(lit);
                }
                // The grammar hands over only its own keys.
                _ => {}
            }
            Ok(())
        })?;

        field.attrs.retain(|a| !a.path().is_ident("expose"));

        let mut wire_default: Option<Option<Expr>> = None;
        for attr in field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("wire_default"))
        {
            if wire_default.is_some() {
                return Err(syn::Error::new_spanned(attr, "duplicate `#[wire_default]`"));
            }
            wire_default = Some(match &attr.meta {
                syn::Meta::Path(_) => None,
                _ => Some(attr.parse_args::<Expr>()?),
            });
        }
        field.attrs.retain(|a| !a.path().is_ident("wire_default"));

        // A type marker without its sea_orm marker is refused: as a scalar it
        // fails inside the `SimpleObject` expansion with a cryptic span.
        let card = relation_cardinality(&ty);
        let relation = match (card, is_belongs_to, is_has_many) {
            (Some((Cardinality::One, target)), true, _) => {
                let from = from_col.ok_or_else(|| {
                    syn::Error::new_spanned(
                        &field.ident,
                        "`belongs_to` relation needs `#[sea_orm(from = \"...\")]`",
                    )
                })?;
                Some(RelationKind::BelongsTo { from, target })
            }
            (Some((Cardinality::Many, target)), _, true) => Some(RelationKind::HasMany {
                target,
                via: via.take(),
            }),
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

        // The `HasMany` arm consumed `via`; one left is misplaced.
        if let Some(via) = via {
            let hint = match &relation {
                Some(RelationKind::BelongsTo { from, .. }) => format!(
                    "a `HasOne` already names its foreign key: `#[sea_orm(belongs_to, from = \"{from}\", …)]`. `via` belongs on the inverse `HasMany` field, which has nothing else to name the column with",
                ),
                _ => "`via` names which of a child's foreign keys a `HasMany` relation follows; this field declares no `HasMany`".to_owned(),
            };
            return Err(syn::Error::new_spanned(&via, hint));
        }

        // `input(...)` on a relation would emit `Set(self.<rel>)` against its
        // marker and fail deep in expansion.
        if relation.is_some() && (in_create || in_update) {
            return Err(syn::Error::new_spanned(
                &field.ident,
                "a relation field cannot be an `input` — expose the scalar FK column (e.g. `org_id`) as the input instead",
            ));
        }

        if wire_default.is_some() {
            if read {
                return Err(syn::Error::new_spanned(
                    &field.ident,
                    "`#[wire_default]` is only valid on an unexposed column — an exposed column reconstructs from the response body; drop `#[wire_default]` or remove `#[expose]`",
                ));
            }
            if is_pk || relation.is_some() {
                return Err(syn::Error::new_spanned(
                    &field.ident,
                    "`#[wire_default]` cannot be applied to a primary key or relation field",
                ));
            }
        }

        fields.push(ResourceField {
            ident,
            ty,
            read,
            in_create,
            in_update,
            is_pk,
            validate,
            relation,
            complexity,
            wire_default,
        });
    }

    if soft_delete {
        let Some(field) = fields.iter().find(|f| f.ident == "deleted_at") else {
            return Err(syn::Error::new_spanned(
                &source_ident,
                "`#[expose(..., soft_delete)]` requires a `deleted_at: Option<…>` column",
            ));
        };
        if !crate::lifecycle::is_option_type(&field.ty) {
            return Err(syn::Error::new_spanned(
                &field.ident,
                "`deleted_at` must be `Option<DateTimeWithTimeZone>` (or similar) for soft delete",
            ));
        }
    }
    if timestamps {
        for name in ["created_at", "updated_at"] {
            fields
                .iter()
                .find(|f| f.ident == name)
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        &source_ident,
                        format!(
                            "`#[expose(..., timestamps)]` requires `{name}` on the entity — remove any manual `impl ActiveModelBehavior` when using this flag",
                        ),
                    )
                })?;
        }
    }

    Ok(ResourceModel {
        source_ident,
        output_ident: name_ident.clone(),
        create_ident: format_ident!("Create{}", name_ident),
        update_ident: format_ident!("Update{}", name_ident),
        fields,
        service,
        complex,
        graphql,
        soft_delete,
        timestamps,
    })
}

/// `#[expose(name = "User")]`'s value: the type the wire DTO is declared as,
/// and the stem of its `Create…` / `Update…` inputs and OpenAPI schema.
/// Refused unless an identifier, since `format_ident!` panics on one that is not.
fn type_name(written: &Expr) -> syn::Result<String> {
    let lit = nest_rs_codegen::require_str_lit(written, "expose", "name", "User")?;
    let name = lit.value();
    if syn::parse_str::<Ident>(&name).is_err() {
        return Err(syn::Error::new_spanned(
            &lit,
            format!(
                "{}: {name:?} is not a Rust identifier — the wire DTO is declared under it, and \
                 its `Create`/`Update` inputs and OpenAPI schema are named from it",
                nest_rs_codegen::site("expose", Some("name")),
            ),
        ));
    }
    Ok(name)
}

/// The entity's own `#[sea_orm(from = "org_id")]`, which `#[expose]` reads to
/// find the field holding a `HasOne` relation's foreign key. The ident keeps the
/// literal's span, so a later refusal of the column lands on the name.
fn foreign_key(written: &Expr) -> syn::Result<Ident> {
    let lit = nest_rs_codegen::require_str_lit(written, "sea_orm", "from", "org_id")?;
    let column = lit.value();
    if syn::parse_str::<Ident>(&column).is_err() {
        return Err(syn::Error::new_spanned(
            &lit,
            format!(
                "{}: {column:?} is not a column name — `#[expose]` reads it as the field holding \
                 this relation's foreign key, e.g. `from = \"org_id\"`",
                nest_rs_codegen::site("sea_orm", Some("from")),
            ),
        ));
    }
    Ok(Ident::new(&column, lit.span()))
}

/// The struct half's key set — the `#[expose]` written on the `Model` — in the
/// order its refusals list them.
const MODEL: Grammar = Grammar::new(
    "expose",
    &[
        "name",
        "service",
        "graphql",
        "soft_delete",
        "timestamps",
        "complex",
    ],
);

/// The field half's key set — the `#[expose]` written on a column, as opposed
/// to the one on the `Model`.
const FIELD: Grammar = Grammar::new("expose", &["input", "validate", "complexity", "via"]);

/// A field key's list value — `input(create, update)` — each element read as
/// written, or the key refused naming what its list takes when no list follows.
fn listed(arg: &Arg<'_>, takes: &str) -> syn::Result<Vec<Expr>> {
    let input = arg.input();
    let refused = |at: proc_macro2::Span| {
        syn::Error::new(
            at,
            nest_rs_codegen::takes_value("expose", Some(arg.key()), takes),
        )
    };
    if !input.peek(syn::token::Paren) {
        return Err(refused(arg.ident().span()));
    }
    let content;
    syn::parenthesized!(content in input);
    let listed = content
        .parse_terminated(Expr::parse, Token![,])
        .map_err(|stopped| refused(stopped.span()))?;
    Ok(listed.into_iter().collect())
}

/// Match `HasOne<T>` / `HasMany<T>` on the last path segment.
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

/// The framework's re-export of async-graphql — the root every emitted derive,
/// attribute and `crate = ` override is pinned to.
pub(crate) fn graphql_root() -> TokenStream2 {
    quote!(::nest_rs_seaorm::__private::async_graphql)
}

/// The same root as the **string** a `crate = ` argument takes, built from
/// [`graphql_root`]'s tokens: a stale string would parse fine and silently send
/// the expansion back to the call site's prelude.
pub(crate) fn graphql_root_str() -> String {
    graphql_root().into_iter().map(|t| t.to_string()).collect()
}

/// The trailing async-graphql derive (`SimpleObject` for output objects,
/// `InputObject` for inputs) to splice into a `#[derive(...)]` list.
pub(crate) fn graphql_object_derive(model: &ResourceModel, derive: &str) -> TokenStream2 {
    if !model.graphql {
        return TokenStream2::new();
    }
    let root = graphql_root();
    let derive = format_ident!("{derive}");
    quote! { #root::#derive, }
}

/// The `crate = ` override those derives need: without it async-graphql roots
/// its expansion in the call site's manifest. `#[ComplexObject]` takes it as an
/// argument (`relations::emit_field_resolvers`).
pub(crate) fn graphql_crate_attr(model: &ResourceModel) -> TokenStream2 {
    if !model.graphql {
        return TokenStream2::new();
    }
    let root = graphql_root_str();
    quote! { #[graphql(crate = #root)] }
}

/// Whether the type's last path segment is `Uuid` (a `String` on the wire).
/// Purely syntactic: `Option<Uuid>` and aliases keep their native type.
pub(crate) fn is_uuid(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Uuid"))
}

/// Whether the type is SeaORM's `DateTimeWithTimeZone`, an RFC 3339 `String` on
/// the wire (async-graphql has no native chrono mapping).
pub(crate) fn is_datetime_tz(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(tp) if tp
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "DateTimeWithTimeZone")
    )
}
