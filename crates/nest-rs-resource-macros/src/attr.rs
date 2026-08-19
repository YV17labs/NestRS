//! Parse `#[expose(...)]` into a [`ResourceModel`] and strip the per-field
//! annotations so the ORM macros see a clean entity.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::Parse;
use syn::{
    Expr, Fields, GenericArgument, Ident, ItemStruct, LitStr, Path, PathArguments, Token, Type,
    TypePath,
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
    /// side only consumes `RelatedTo<Self::Entity, Via>::Loader`.
    HasMany {
        /// `crate::users::Entity`.
        target: Path,
        /// `#[expose(via = "author_id")]` — which of the child's foreign keys
        /// this relation follows. `None` ⇒ the child's sole key to this parent
        /// (`SoleForeignKey`), which is a compile error when it has two. Kept
        /// as the [`LitStr`] the developer wrote so a bad column name reports
        /// at the string, not at the field.
        via: Option<LitStr>,
    },
}

pub(crate) struct ResourceField {
    pub ident: Ident,
    pub ty: Type,
    /// `true` when the field carries `#[expose]` in any form — it then appears
    /// in the wire / GraphQL output, and a relation gets its auto field
    /// resolver. A field with **no** `#[expose]` is hidden from every transport.
    /// Exposure is opt-in: silence means hidden, never leaked.
    pub read: bool,
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
    /// Override async-graphql's per-field complexity for the auto-emitted
    /// field resolver. Accepts a literal (`complexity = 5`) or an expression
    /// string (`complexity = "first * child_complexity"`). When `None`, the
    /// macro picks a safe default per relation kind (see `relations::emit`).
    pub complexity: Option<Expr>,
    /// Audited opt-in for the masking placeholder of an **unexposed** column
    /// whose type the emitter can't default (custom enum, `Uuid`, timestamp,
    /// `Decimal`). `None` ⇒ absent; `Some(None)` ⇒ bare `#[wire_default]` (the
    /// column type's `Default`); `Some(Some(expr))` ⇒ `#[wire_default(expr)]`.
    /// See `wire.rs` for the safety contract (only sound when no `Ability` rule
    /// predicates on the column, since the placeholder is stripped before the
    /// body ships).
    pub wire_default: Option<Option<Expr>>,
}

impl ResourceField {
    /// True iff the field belongs in the output struct as a plain column. A
    /// relation never does — it is materialised by a `#[ComplexObject]` field
    /// resolver (or skipped entirely).
    pub fn in_output_struct(&self) -> bool {
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
    /// Emit `#[graphql(complex)]` on the output. Set explicitly via
    /// `complex` or implicitly when any exposed relation calls for a
    /// `#[ComplexObject]`.
    pub complex: bool,
    /// When set, emit GraphQL surface types (SimpleObject, loaders, relations).
    pub graphql: bool,
    /// Stamp `deleted_at` instead of hard-deleting; emit [`SoftDeletable`].
    pub soft_delete: bool,
    /// Maintain `created_at` / `updated_at` via `ActiveModelBehavior::before_save`.
    pub timestamps: bool,
}

impl ResourceModel {
    /// True iff at least one exposed (`#[expose]`) relation needs a `#[ComplexObject]`.
    pub fn has_auto_relations(&self) -> bool {
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
    // A repeat is refused here for the reason `args.rs` states once: "Accepting
    // the repeat means dropping one of two declarations, and which one it drops
    // is source order." On this decorator that is the **wire**:
    // `#[expose(name = "User", name = "Account")]` compiled, and the DTO and the
    // OpenAPI schema took whichever came last.
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            nest_rs_codegen::once(name.is_some(), &meta.path, "expose", "name")?;
            // A bare `#[expose(name)]` reaches `meta.value()` as syn's
            // `` expected `=` ``, which names the grammar and not the key.
            if !meta.input.peek(syn::Token![=]) {
                return Err(meta.error(nest_rs_codegen::needs_a_value("expose", "name")));
            }
            name = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("service") {
            nest_rs_codegen::once(service.is_some(), &meta.path, "expose", "service")?;
            if !meta.input.peek(syn::Token![=]) {
                return Err(meta.error(nest_rs_codegen::needs_a_value("expose", "service")));
            }
            service = Some(meta.value()?.parse::<Path>()?);
            Ok(())
        } else if meta.path.is_ident("complex") {
            nest_rs_codegen::once(complex, &meta.path, "expose", "complex")?;
            complex = true;
            Ok(())
        } else if meta.path.is_ident("graphql") {
            nest_rs_codegen::once(graphql, &meta.path, "expose", "graphql")?;
            graphql = true;
            Ok(())
        } else if meta.path.is_ident("soft_delete") {
            nest_rs_codegen::once(soft_delete, &meta.path, "expose", "soft_delete")?;
            soft_delete = true;
            Ok(())
        } else if meta.path.is_ident("timestamps") {
            nest_rs_codegen::once(timestamps, &meta.path, "expose", "timestamps")?;
            timestamps = true;
            Ok(())
        } else {
            Err(meta.error(nest_rs_codegen::unknown_argument(
                "expose",
                &nest_rs_codegen::key_as_written(&meta.path),
                &[
                    "name",
                    "service",
                    "graphql",
                    "soft_delete",
                    "timestamps",
                    "complex",
                ],
            )))
        }
    });
    syn::parse::Parser::parse2(parser, args)?;

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
        let ident = field.ident.clone().expect("named field has an ident");
        let ty = field.ty.clone();
        let mut read = false;
        let mut in_create = false;
        let mut in_update = false;
        let mut validate = Vec::new();
        let mut complexity: Option<Expr> = None;
        let mut via: Option<LitStr> = None;

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

        // Exposure is opt-in: the mere presence of `#[expose]` (bare or with
        // options) marks the field for read exposure; `input(...)` additionally
        // opts it into the write DTOs (and so implies read). A field carrying
        // no `#[expose]` is hidden from every transport — silence is never a
        // leak. A column added by a later migration stays invisible until
        // someone deliberately exposes it.
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("expose")) {
            read = true;
            // Bare `#[expose]` (no parens) carries no options — nothing to parse.
            if matches!(attr.meta, syn::Meta::Path(_)) {
                continue;
            }
            attr.parse_nested_meta(|m| {
                if m.path.is_ident("input") {
                    let content;
                    syn::parenthesized!(content in m.input);
                    let kinds = content.parse_terminated(Ident::parse, Token![,])?;
                    for k in kinds {
                        if k == "create" {
                            in_create = true;
                        } else if k == "update" {
                            in_update = true;
                        } else {
                            return Err(syn::Error::new(
                                k.span(),
                                nest_rs_codegen::unknown_value(
                                    "expose",
                                    "input",
                                    &k.to_string(),
                                    &["create", "update"],
                                ),
                            ));
                        }
                    }
                } else if m.path.is_ident("validate") {
                    let content;
                    syn::parenthesized!(content in m.input);
                    validate.push(content.parse()?);
                } else if m.path.is_ident("complexity") {
                    // Accepts a literal int (`complexity = 5`) or an expression
                    // string async-graphql parses (`complexity = "first.unwrap_or(20)
                    // as usize * child_complexity"`) — both re-emit verbatim
                    // into the generated `#[graphql(complexity = ...)]`. A
                    // `HasMany` resolver takes `first`/`after`, so the
                    // expression may name them; every other field has no
                    // arguments to name.
                    complexity = Some(m.value()?.parse::<Expr>()?);
                } else if m.path.is_ident("via") {
                    // Which of the child's foreign keys a `HasMany` follows.
                    // A column name, not a path: the marker type the parent
                    // resolves it to is the framework's business.
                    let lit = m.value()?.parse::<LitStr>()?;
                    if syn::parse_str::<Ident>(&lit.value()).is_err() {
                        return Err(syn::Error::new_spanned(
                            &lit,
                            "`via` takes a snake_case column name on the child entity (e.g. `via = \"author_id\"`)",
                        ));
                    }
                    via = Some(lit);
                } else {
                    return Err(m.error(nest_rs_codegen::unknown_argument(
                        "expose",
                        &nest_rs_codegen::key_as_written(&m.path),
                        &["input", "validate", "complexity", "via"],
                    )));
                }
                Ok(())
            })?;
        }

        field.attrs.retain(|a| !a.path().is_ident("expose"));

        // The audited masking-placeholder opt-in. Bare `#[wire_default]` emits
        // the column type's `Default`; `#[wire_default(expr)]` emits `expr`.
        // Meaningful only for a column the wire DTO omits, so misuse on an
        // exposed / PK / relation field is a hard error below, not a silent
        // no-op. Strip it so the ORM derives never see it.
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

        // `via` picks between a child's foreign keys, so it is meaningful on
        // exactly one shape. The `HasMany` arm above consumed it; anything left
        // is on a `HasOne` — which already names its column in
        // `#[sea_orm(from = …)]`, and two spellings of one fact is the "one way
        // to do a thing" rule — or on a plain column, where it means nothing.
        if let Some(via) = via {
            let hint = match &relation {
                Some(RelationKind::BelongsTo { from, .. }) => format!(
                    "a `HasOne` already names its foreign key: `#[sea_orm(belongs_to, from = \"{from}\", …)]`. `via` belongs on the inverse `HasMany` field, which has nothing else to name the column with",
                ),
                _ => "`via` names which of a child's foreign keys a `HasMany` relation follows; this field declares no `HasMany`".to_owned(),
            };
            return Err(syn::Error::new_spanned(&via, hint));
        }

        // A relation is materialised by a field resolver, not a column setter —
        // `input(...)` on it would emit `__am.<rel> = Set(self.<rel>)` against a
        // `HasOne`/`HasMany` marker and fail deep in expansion. Refuse early.
        if relation.is_some() && (in_create || in_update) {
            return Err(syn::Error::new_spanned(
                &field.ident,
                "a relation field cannot be an `input` — expose the scalar FK column (e.g. `org_id`) as the input instead",
            ));
        }

        // `#[wire_default]` is only meaningful for a column the wire DTO omits.
        // An exposed column reconstructs from the body; a PK is never
        // fabricated; a relation is materialised by a field resolver. Refusing
        // these loudly (solid > silent) keeps the placeholder auditable.
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

/// The framework's re-export of async-graphql — the root every emitted derive,
/// attribute and `crate = ` override is pinned to.
pub(crate) fn graphql_root() -> TokenStream2 {
    quote!(::nest_rs_resource::graphql::async_graphql)
}

/// The same root as the **string** a `crate = ` argument takes, built from
/// [`graphql_root`]'s tokens rather than re-typed.
///
/// The two forms must not drift: a path that no longer resolves is a compile
/// error at the emit site, while a stale string parses fine and silently sends
/// the expansion back to the call site's prelude — the failure this override
/// exists to close.
pub(crate) fn graphql_root_str() -> String {
    graphql_root().into_iter().map(|t| t.to_string()).collect()
}

/// The trailing async-graphql derive (`SimpleObject` for output objects,
/// `InputObject` for inputs) to splice into a `#[derive(...)]` list — present
/// only when `#[expose(graphql)]` is on, empty otherwise.
pub(crate) fn graphql_object_derive(model: &ResourceModel, derive: &str) -> TokenStream2 {
    if !model.graphql {
        return TokenStream2::new();
    }
    let root = graphql_root();
    let derive = format_ident!("{derive}");
    quote! { #root::#derive, }
}

/// The `crate = ` override those derives need.
///
/// An async-graphql derive roots its own expansion at whatever
/// `proc-macro-crate` finds in the *call site's* manifest, falling back to a
/// bare `::async_graphql`. Without this the entity crate would have to declare
/// `async-graphql` — and pin its version by hand — for code it never wrote;
/// same reason `#[expose]` already spells out serde's and schemars' overrides.
/// The emitted `#[ComplexObject]` needs the same override, spelled as an
/// argument rather than an attribute — see `relations::emit_field_resolvers`.
pub(crate) fn graphql_crate_attr(model: &ResourceModel) -> TokenStream2 {
    if !model.graphql {
        return TokenStream2::new();
    }
    let root = graphql_root_str();
    quote! { #[graphql(crate = #root)] }
}

/// `true` when the type's last path segment is `Uuid` (rendered as `String` on
/// the GraphQL output). Purely syntactic: `Option<Uuid>` and aliases pass
/// through with their native type.
pub(crate) fn is_uuid(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Uuid"))
}

/// `true` for SeaORM's `DateTimeWithTimeZone` — rendered as RFC 3339 `String`
/// on the wire and in GraphQL (async-graphql has no native chrono mapping).
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
