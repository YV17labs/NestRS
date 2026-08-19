//! Shared parser for `#[crud(...)]`, consumed by the HTTP and GraphQL CRUD
//! generators.
//!
//! The grammar is the same on both surfaces, and it is the *whole* grammar on
//! both: every key [`CrudConfig`] carries is read by each generator. The
//! sentence here used to promise otherwise — "REST consumes `guards`; GraphQL
//! ignores them", about a `guards` key that has never existed — and the second
//! half is the shape `CLAUDE.md` bans, written as though it were the design. A
//! key one surface cannot honour is a compile error naming the fact, never a
//! field quietly dropped.

use proc_macro2::{Span, TokenStream as TokenStream2};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, Path, Token};

/// How a generated `list` op bounds its result set.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Paginate {
    /// Keyset over the primary key — the default. Free for UUID-v7 keys
    /// (ordered).
    Cursor,
    /// Explicit opt-out: the full (ability-scoped) collection in one
    /// response, still backstopped by `CrudService::list`'s hard cap.
    None,
}

/// One CRUD operation a `#[crud]` block may generate. The write ops
/// (`Create`/`Update`/`Delete`) each require the resource to implement the
/// matching opt-in trait (`Creatable`/`Updatable`/`Deletable`); `Create`/`Update`
/// additionally require an input type (`create = ` / `update = `).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CrudOp {
    /// `GET /` — the collection, bounded by [`Paginate`].
    List,
    /// `GET /{id}` — one resource by primary key.
    Get,
    /// `POST /` — needs `create = <InputType>` and `Creatable`.
    Create,
    /// `PATCH /{id}` — needs `update = <InputType>` and `Updatable`.
    Update,
    /// `DELETE /{id}` — needs `Deletable`.
    Delete,
}

/// Which operations a `#[crud]` block generates.
pub enum OpsSelection {
    /// No `ops = [...]` given. Back-compatible auto mode: `list` + `get` +
    /// `delete` always, plus `create`/`update` when their input type is given.
    Default,
    /// Explicit `ops = [...]`: exactly the listed ops, validated against the
    /// input types that are present. Carries the `ops` key span for diagnostics.
    Explicit(Vec<CrudOp>, Span),
}

/// Resolved per-op generation decision — the answer the generators consume.
/// The write ops that carry an input type expose it directly (`Some(path)` ⇒
/// generate, borrowing it for the emit) so a generator never re-reaches into
/// `CrudConfig` nor re-asserts the "type is present" invariant.
pub struct GeneratedOps<'a> {
    /// Generate the collection read.
    pub list: bool,
    /// Generate the by-id read.
    pub get: bool,
    /// The create-input type when the op is generated, `None` when it is not.
    pub create: Option<&'a Path>,
    /// The update-input type when the op is generated, `None` when it is not.
    pub update: Option<&'a Path>,
    /// Generate the delete.
    pub delete: bool,
}

/// The parsed `#[crud(...)]` configuration both surface generators consume.
pub struct CrudConfig {
    /// Field holding the entity's `CrudService` — every generated op
    /// delegates to it so controllers/resolvers never touch `Repo` directly.
    pub service: Ident,
    /// The SeaORM entity the operations target.
    pub entity: Path,
    /// The `#[expose]` wire DTO returned by the generated read ops.
    pub output: Path,
    /// The create-input type (`create = `); `None` disables the `create` op.
    pub create: Option<Path>,
    /// The update-input type (`update = `); `None` disables the `update` op.
    pub update: Option<Path>,
    /// Which operations to generate (default = all five, back-compatibly).
    pub ops: OpsSelection,
    /// How the generated list op bounds its result set. Defaults to
    /// [`Paginate::Cursor`] — an unbounded list is an explicit opt-out
    /// (`paginate = none`), never the silent default.
    pub paginate: Paginate,
}

impl CrudConfig {
    /// Resolve which ops to generate, validating that any explicitly requested
    /// `create`/`update` op has its input type. A `create`/`update` op without
    /// `create = ` / `update = ` is a hard error — never a silently dropped op.
    pub fn generated_ops(&self) -> syn::Result<GeneratedOps<'_>> {
        match &self.ops {
            OpsSelection::Default => Ok(GeneratedOps {
                list: true,
                get: true,
                create: self.create.as_ref(),
                update: self.update.as_ref(),
                delete: true,
            }),
            OpsSelection::Explicit(ops, span) => {
                let wants = |op| ops.contains(&op);
                Ok(GeneratedOps {
                    list: wants(CrudOp::List),
                    get: wants(CrudOp::Get),
                    create: resolve_write_op(
                        wants(CrudOp::Create),
                        self.create.as_ref(),
                        *span,
                        "create",
                        "Creatable",
                    )?,
                    update: resolve_write_op(
                        wants(CrudOp::Update),
                        self.update.as_ref(),
                        *span,
                        "update",
                        "Updatable",
                    )?,
                    delete: wants(CrudOp::Delete),
                })
            }
        }
    }
}

/// A write op that carries an input type generates only when that type is
/// present — its absence (when the op was explicitly requested) is a hard
/// error, not a silently dropped op.
fn resolve_write_op<'a>(
    wanted: bool,
    ty: Option<&'a Path>,
    span: Span,
    key: &str,
    trait_name: &str,
) -> syn::Result<Option<&'a Path>> {
    if wanted && ty.is_none() {
        return Err(syn::Error::new(
            span,
            format!(
                "#[crud] `ops` lists `{key}` but no `{key} = <InputType>` was given — a resource \
                 generates `{key}` only when it provides the input type and implements \
                 `{trait_name}`"
            ),
        ));
    }
    Ok(if wanted { ty } else { None })
}

/// Every key `#[crud]` takes, in declaration order — the list the unknown-key
/// refusal reads, so adding a key cannot leave the sentence behind.
const KEYS: [&str; 7] = [
    "service", "entity", "output", "create", "update", "ops", "paginate",
];

/// Refuse a bare key. `expected `=`` is syn's, and it names the grammar rather
/// than the key the developer wrote — the third of the three refusals a
/// `key = value` grammar owes, worded once in `nest_rs_codegen::args`.
fn value_for(input: ParseStream, key: &Ident) -> syn::Result<()> {
    if input.parse::<Token![=]>().is_err() {
        return Err(syn::Error::new(
            key.span(),
            crate::needs_a_value("crud", &key.to_string()),
        ));
    }
    Ok(())
}

impl Parse for CrudConfig {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut service = None;
        let mut entity = None;
        let mut output = None;
        let mut create = None;
        let mut update = None;
        let mut ops = OpsSelection::Default;
        let mut paginate = Paginate::Cursor;
        let mut paginate_declared = false;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "service" => {
                    crate::once(service.is_some(), &key, "crud", &key.to_string())?;
                    value_for(input, &key)?;
                    service = Some(input.parse()?);
                }
                "entity" => {
                    crate::once(entity.is_some(), &key, "crud", &key.to_string())?;
                    value_for(input, &key)?;
                    entity = Some(input.parse()?);
                }
                "output" => {
                    crate::once(output.is_some(), &key, "crud", &key.to_string())?;
                    value_for(input, &key)?;
                    output = Some(input.parse()?);
                }
                "create" => {
                    crate::once(create.is_some(), &key, "crud", &key.to_string())?;
                    value_for(input, &key)?;
                    create = Some(input.parse()?);
                }
                "update" => {
                    crate::once(update.is_some(), &key, "crud", &key.to_string())?;
                    value_for(input, &key)?;
                    update = Some(input.parse()?);
                }
                "ops" => {
                    if !matches!(ops, OpsSelection::Default) {
                        return Err(syn::Error::new(
                            key.span(),
                            crate::duplicate_argument("crud", "ops"),
                        ));
                    }
                    let ops_span = key.span();
                    value_for(input, &key)?;
                    let content;
                    syn::bracketed!(content in input);
                    let idents = content.parse_terminated(Ident::parse, Token![,])?;
                    let mut selected = Vec::new();
                    for id in idents {
                        let op = match id.to_string().as_str() {
                            "list" => CrudOp::List,
                            "get" => CrudOp::Get,
                            "create" => CrudOp::Create,
                            "update" => CrudOp::Update,
                            "delete" => CrudOp::Delete,
                            other => {
                                return Err(syn::Error::new(
                                    id.span(),
                                    crate::unknown_value(
                                        "crud",
                                        "op",
                                        other,
                                        &["list", "get", "create", "update", "delete"],
                                    ),
                                ));
                            }
                        };
                        selected.push(op);
                    }
                    if selected.is_empty() {
                        // The same answer `version = []` gives, and it had the
                        // other one: an empty list generated a `#[crud]` block
                        // with no operations in it and said nothing, while the
                        // field's own doc calls an unbounded list "an explicit
                        // opt-out, never the silent default".
                        return Err(syn::Error::new(
                            ops_span,
                            "#[crud] `ops = []` declares nothing — drop the argument to \
                             generate the default set, or list the operations you want",
                        ));
                    }
                    ops = OpsSelection::Explicit(selected, ops_span);
                }
                "paginate" => {
                    // The one arm whose slot is not an `Option`, which is why it
                    // was the one with no refusal. A dropped second declaration
                    // here reverses `paginate = none` — the explicit opt-out
                    // into an unbounded list — in either direction.
                    if paginate_declared {
                        return Err(syn::Error::new(
                            key.span(),
                            crate::duplicate_argument("crud", "paginate"),
                        ));
                    }
                    paginate_declared = true;
                    value_for(input, &key)?;
                    let mode: Ident = input.parse()?;
                    paginate = match mode.to_string().as_str() {
                        "cursor" => Paginate::Cursor,
                        "none" => Paginate::None,
                        other => {
                            return Err(syn::Error::new(
                                mode.span(),
                                crate::unknown_value(
                                    "crud",
                                    "paginate",
                                    other,
                                    &["cursor", "none"],
                                ),
                            ));
                        }
                    };
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        crate::unknown_argument("crud", other, &KEYS),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        // Three required keys, one sentence — the crate that owns the wording
        // was itself three of the family's eight hand-written copies.
        let service = service.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                format!(
                    "{} (the injected `CrudService` field to delegate to)",
                    crate::missing_argument("crud", "service", "svc"),
                ),
            )
        })?;
        let entity = entity.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                crate::missing_argument("crud", "entity", "users::Entity"),
            )
        })?;
        let output = output.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                crate::missing_argument("crud", "output", "User"),
            )
        })?;

        Ok(CrudConfig {
            service,
            entity,
            output,
            create,
            update,
            ops,
            paginate,
        })
    }
}

/// Parse a `#[crud(...)]` attribute's tokens into a [`CrudConfig`].
pub fn parse_crud_args(args: TokenStream2) -> syn::Result<CrudConfig> {
    syn::parse2(args)
}

/// Snake-cased last segment of the output type (`User` → `user`,
/// `ArtistExhibition` → `artist_exhibition`); base for generated operation
/// method names (the list op is `<base>s`). async-graphql camelCases the method
/// ident, so snake_case — not a bare lowercase — is what lets a compound entity
/// reach `createArtistExhibition`; flattening the word boundaries to
/// `artistexhibition` strands it at `createArtistexhibition`.
///
/// Pluralization stays naive, **not** real singularization/pluralization: an
/// irregular or already-plural entity yields an ungrammatical op name
/// (`Category` → list op `categorys`, `Person` → `persons`). When that matters,
/// hand-write the operation — `#[crud]` skips generating any op a method of the
/// same name already defines.
pub fn singular_of(output: &Path) -> String {
    output
        .segments
        .last()
        .map(|s| crate::snake_case(&s.ident.to_string()))
        .unwrap_or_else(|| "item".to_owned())
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    fn parse(args: proc_macro2::TokenStream) -> syn::Result<CrudConfig> {
        parse_crud_args(args)
    }

    // A compound PascalCase entity must keep its word boundaries: async-graphql
    // camelCases the generated method ident, so `create_artist_exhibition`
    // becomes `createArtistExhibition`. A flat lowercase collapsed it to
    // `artistexhibition`, stranding the op at `createArtistexhibition`.
    #[test]
    fn singular_of_snake_cases_compound_entity_names() {
        let compound: syn::Path = syn::parse_quote!(ArtistExhibition);
        assert_eq!(singular_of(&compound), "artist_exhibition");
        // Single-word entities are unchanged — no schema churn for `users` &co.
        let single: syn::Path = syn::parse_quote!(User);
        assert_eq!(singular_of(&single), "user");
    }

    // No `ops` ⇒ back-compatible auto mode: with both input types present every
    // op is generated, so existing `#[crud(create = .., update = ..)]` sites are
    // unchanged.
    #[test]
    fn default_with_both_inputs_generates_all_five() {
        let cfg = parse(quote! {
            service = svc, entity = E, output = O, create = C, update = U
        })
        .expect("parses");
        let ops = cfg.generated_ops().expect("resolves");
        assert!(ops.list && ops.get && ops.delete);
        assert!(ops.create.is_some() && ops.update.is_some());
    }

    // Auto mode without input types: list/get/delete (delete needs no type),
    // never create/update — today's behaviour, preserved.
    #[test]
    fn default_without_inputs_skips_create_and_update() {
        let cfg = parse(quote! { service = svc, entity = E, output = O }).expect("parses");
        let ops = cfg.generated_ops().expect("resolves");
        assert!(ops.list && ops.get && ops.delete);
        assert!(ops.create.is_none() && ops.update.is_none());
    }

    // Explicit selection generates exactly the listed ops — and needs no
    // `create`/`update` input type when those ops are not requested.
    #[test]
    fn explicit_partial_selection_generates_only_listed_ops() {
        let cfg = parse(quote! {
            service = svc, entity = E, output = O, ops = [list, get, delete]
        })
        .expect("parses");
        let ops = cfg.generated_ops().expect("resolves");
        assert!(ops.list && ops.get && ops.delete);
        assert!(ops.create.is_none() && ops.update.is_none());
    }

    // Requesting `create` without `create = <Type>` is a hard error, not a
    // silently dropped (or no-op) operation.
    #[test]
    fn explicit_create_without_input_type_is_an_error() {
        let cfg = parse(quote! {
            service = svc, entity = E, output = O, ops = [list, create]
        })
        .expect("parses");
        let err = match cfg.generated_ops() {
            Ok(_) => panic!("create without an input type must fail"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("create"));
    }

    // The same guard for `update`.
    #[test]
    fn explicit_update_without_input_type_is_an_error() {
        let cfg = parse(quote! {
            service = svc, entity = E, output = O, ops = [update]
        })
        .expect("parses");
        let err = match cfg.generated_ops() {
            Ok(_) => panic!("update without an input type must fail"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("update"));
    }

    // With the input type present, the requested write op resolves.
    #[test]
    fn explicit_create_with_input_type_resolves() {
        let cfg = parse(quote! {
            service = svc, entity = E, output = O, create = C, ops = [get, create]
        })
        .expect("parses");
        let ops = cfg.generated_ops().expect("resolves");
        assert!(ops.get && ops.create.is_some());
        assert!(!ops.list && ops.update.is_none() && !ops.delete);
    }

    // An unknown op name is rejected at parse time.
    #[test]
    fn unknown_op_name_is_rejected() {
        let err = match parse(quote! {
            service = svc, entity = E, output = O, ops = [list, frobnicate]
        }) {
            Ok(_) => panic!("unknown op must fail to parse"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("frobnicate"));
    }
}
