//! Shared parser for `#[crud(...)]`, consumed by the HTTP and GraphQL CRUD
//! generators. The grammar is the same on both surfaces; each generator reads
//! the fields it cares about (REST consumes `guards`; GraphQL ignores them).

use proc_macro2::{Span, TokenStream as TokenStream2};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, Path, Token};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Paginate {
    /// Keyset over the primary key — the default. Free for UUID-v7 keys
    /// (ordered).
    Cursor,
    /// Offset (`page`/`per_page`). Random access at the cost of O(offset)
    /// scans and instability under concurrent inserts.
    Page,
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
    List,
    Get,
    Create,
    Update,
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
    pub list: bool,
    pub get: bool,
    pub create: Option<&'a Path>,
    pub update: Option<&'a Path>,
    pub delete: bool,
}

pub struct CrudConfig {
    /// Field holding the entity's [`CrudService`] — every generated op
    /// delegates to it so controllers/resolvers never touch `Repo` directly.
    pub service: Ident,
    pub entity: Path,
    pub output: Path,
    pub create: Option<Path>,
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

impl Parse for CrudConfig {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut service = None;
        let mut entity = None;
        let mut output = None;
        let mut create = None;
        let mut update = None;
        let mut ops = OpsSelection::Default;
        let mut paginate = Paginate::Cursor;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "service" => {
                    input.parse::<Token![=]>()?;
                    service = Some(input.parse()?);
                }
                "entity" => {
                    input.parse::<Token![=]>()?;
                    entity = Some(input.parse()?);
                }
                "output" => {
                    input.parse::<Token![=]>()?;
                    output = Some(input.parse()?);
                }
                "create" => {
                    input.parse::<Token![=]>()?;
                    create = Some(input.parse()?);
                }
                "update" => {
                    input.parse::<Token![=]>()?;
                    update = Some(input.parse()?);
                }
                "ops" => {
                    let ops_span = key.span();
                    input.parse::<Token![=]>()?;
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
                                    format!(
                                        "unknown #[crud] op `{other}` (expected `list`, `get`, \
                                         `create`, `update`, `delete`)"
                                    ),
                                ));
                            }
                        };
                        selected.push(op);
                    }
                    ops = OpsSelection::Explicit(selected, ops_span);
                }
                "paginate" => {
                    input.parse::<Token![=]>()?;
                    let mode: Ident = input.parse()?;
                    paginate = match mode.to_string().as_str() {
                        "cursor" => Paginate::Cursor,
                        "page" => Paginate::Page,
                        "none" => Paginate::None,
                        _ => {
                            return Err(syn::Error::new(
                                mode.span(),
                                "expected `cursor`, `page`, or `none`",
                            ));
                        }
                    };
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown #[crud] option `{other}` (expected `service`, `entity`, \
                             `output`, `create`, `update`, `ops`, `paginate`)"
                        ),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        let service = service.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "#[crud] requires `service = <field>` (the injected CrudService to delegate to)",
            )
        })?;
        let entity = entity.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "#[crud] requires `entity = ...::Entity`")
        })?;
        let output = output.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "#[crud] requires `output = OutputType`")
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
