//! Shared parser for `#[crud(...)]`, consumed by the HTTP and GraphQL CRUD
//! generators; each reads every key [`CrudDeclaration`] carries.

use proc_macro2::{Span, TokenStream as TokenStream2};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Path, Token};

use crate::grammar::Grammar;

/// How a generated `list` op bounds its result set.
#[derive(Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "tier 2: `#[crud]`'s HTTP and GraphQL emitters match it, and a new mode must reach both"
)]
pub enum Paginate {
    /// Keyset over the primary key — the default.
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
#[non_exhaustive]
pub enum CrudOp {
    /// `GET /` — the collection, bounded by [`Paginate`].
    List,
    /// `GET /{id}` — one resource by primary key.
    Get,
    /// `POST /` — needs `create = <InputType>` and `Creatable`.
    Create,
    /// `PUT /{id}` — needs `update = <InputType>` and `Updatable`.
    Update,
    /// `DELETE /{id}` — needs `Deletable`.
    Delete,
}

/// What `api(<op>(summary = "…", description = "…"))` documents one generated
/// operation with, each `None` keeping the default.
#[derive(Default)]
#[non_exhaustive]
pub struct CrudOpApi {
    /// The operation's OpenAPI `summary`.
    pub summary: Option<LitStr>,
    /// The operation's OpenAPI `description`.
    pub description: Option<LitStr>,
}

/// Which operations a `#[crud]` block generates.
#[non_exhaustive]
pub enum OpsSelection {
    /// No `ops = [...]` given: `list` + `get` + `delete` always, plus
    /// `create`/`update` when their input type is given.
    Default,
    /// Explicit `ops = [...]`: exactly the listed ops, validated against the
    /// input types that are present. Carries the `ops` key span for diagnostics.
    Explicit(Vec<CrudOp>, Span),
}

/// Resolved per-op generation decision — the answer the generators consume;
/// a write op is generated when its input type is `Some`.
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

/// The parsed `#[crud(...)]` declaration both surface generators consume.
pub struct CrudDeclaration {
    /// Field holding the entity's `CrudService`, which every generated op
    /// delegates to.
    pub service: Ident,
    /// The SeaORM entity the operations target.
    pub entity: Path,
    /// The `#[expose]` wire DTO returned by the generated read ops.
    pub output: Path,
    /// The create-input type (`create = `); `None` disables the `create` op.
    pub create: Option<Path>,
    /// The update-input type (`update = `); `None` disables the `update` op.
    pub update: Option<Path>,
    /// Which operations to generate.
    pub ops: OpsSelection,
    /// How the generated list op bounds its result set. Defaults to
    /// [`Paginate::Cursor`] — an unbounded list is an explicit opt-out
    /// (`paginate = none`), never the silent default.
    pub paginate: Paginate,
    /// Where `paginate` was written, for a refusal when `ops` leaves `list` out.
    paginate_written: Option<Span>,
    /// `api(…)`: each documented operation, its docs and where it was named.
    api: Vec<(CrudOp, CrudOpApi, Span)>,
    /// Where `api` was written, for an edge that documents no operation.
    api_written: Option<Span>,
}

impl CrudDeclaration {
    /// The docs `api(…)` gives `op`, when it names it.
    pub fn api(&self, op: CrudOp) -> Option<&CrudOpApi> {
        self.api
            .iter()
            .find_map(|(named, api, _)| (*named == op).then_some(api))
    }

    /// Where `api(…)` was written, if it was.
    pub fn api_written(&self) -> Option<Span> {
        self.api_written
    }

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
                if let (false, Some(written)) = (wants(CrudOp::List), self.paginate_written) {
                    return Err(excluded_op_key(written, "paginate", "list", "paginate = …"));
                }
                if let Some((op, _, written)) = self.api.iter().find(|(op, ..)| !wants(*op)) {
                    let name = op_name(*op);
                    return Err(excluded_op_key(
                        *written,
                        "api",
                        name,
                        &format!("{name}(…)"),
                    ));
                }
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

/// A requested write op without its input type, and an input type for an op
/// `ops` leaves out, are both refused — never a silently dropped declaration.
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
                "{} lists `{key}` but no `{key} = <InputType>` was given — a resource generates \
                 `{key}` only when it provides the input type and implements `{trait_name}`",
                crate::args::site("crud", Some("ops")),
            ),
        ));
    }
    if let (false, Some(ty)) = (wanted, ty) {
        return Err(syn::Error::new_spanned(
            ty,
            format!(
                "{} names the input of an op `ops` leaves out — list `{key}` in `ops` to \
                 generate it, or drop `{key} = …`",
                crate::args::site("crud", Some(key)),
            ),
        ));
    }
    Ok(ty)
}

/// The refusal of a key that configures one op, declared beside an `ops` that
/// leaves that op out; `create`/`update` go through [`resolve_write_op`].
fn excluded_op_key(written: Span, key: &str, op: &str, drop: &str) -> syn::Error {
    syn::Error::new(
        written,
        format!(
            "{} configures the `{op}` op, which `ops` leaves out — list `{op}` in `ops` to \
             generate it, or drop `{drop}`",
            crate::args::site("crud", Some(key)),
        ),
    )
}

/// Every key `#[crud]` takes, in declaration order.
const CRUD: Grammar = Grammar::new(
    "crud",
    &[
        "service", "entity", "output", "create", "update", "ops", "paginate", "api",
    ],
);

/// The operations `api(…)` documents, named as `ops` names them.
const CRUD_API: Grammar =
    Grammar::new("crud", &["list", "get", "create", "update", "delete"]).under("api");

/// What `api(<op>(…))` sets on one operation.
const CRUD_OP_API: Grammar = Grammar::new("crud", &["summary", "description"]).under("api");

const API_LIST: &str = "each operation's docs, e.g. `api(get(summary = \"Get a user by id\"))`";

/// An operation by the name `ops` and `api` give it.
fn op_named(name: &str) -> Option<CrudOp> {
    match name {
        "list" => Some(CrudOp::List),
        "get" => Some(CrudOp::Get),
        "create" => Some(CrudOp::Create),
        "update" => Some(CrudOp::Update),
        "delete" => Some(CrudOp::Delete),
        _ => None,
    }
}

/// The name `ops` and `api` give `op`.
fn op_name(op: CrudOp) -> &'static str {
    match op {
        CrudOp::List => "list",
        CrudOp::Get => "get",
        CrudOp::Create => "create",
        CrudOp::Update => "update",
        CrudOp::Delete => "delete",
    }
}

/// `api(get(summary = "…"), list(description = "…"))`, read at `arg`.
fn parse_api(arg: &crate::grammar::Arg<'_>) -> syn::Result<Vec<(CrudOp, CrudOpApi, Span)>> {
    let input = arg.input();
    if !input.peek(syn::token::Paren) {
        return Err(syn::Error::new(
            arg.ident().span(),
            crate::args::takes_value("crud", Some("api"), API_LIST),
        ));
    }
    let content;
    syn::parenthesized!(content in input);
    let mut documented = Vec::new();
    CRUD_API.parse(&content, |op_arg| {
        let Some(op) = op_named(op_arg.key()) else {
            return Ok(());
        };
        let input = op_arg.input();
        if !input.peek(syn::token::Paren) {
            return Err(syn::Error::new(
                op_arg.ident().span(),
                crate::args::takes_value("crud", Some("api"), API_LIST),
            ));
        }
        let docs;
        syn::parenthesized!(docs in input);
        let mut api = CrudOpApi::default();
        CRUD_OP_API.parse(&docs, |doc| {
            match doc.key() {
                "summary" => api.summary = Some(doc.str_lit("Get a user by id")?),
                "description" => api.description = Some(doc.str_lit("…")?),
                // The grammar hands over only its own keys.
                _ => {}
            }
            Ok(())
        })?;
        documented.push((op, api, op_arg.ident().span()));
        Ok(())
    })?;
    Ok(documented)
}

const PAGINATE: [&str; 2] = ["cursor", "none"];

const OPS_LIST: &str = "a list of operations, e.g. `ops = [list, get]`";

/// Parse a key's value, refusing one of the wrong kind with the shared value
/// sentence rather than syn's `expected identifier`.
fn value_of<T: Parse>(input: ParseStream, key: &str, what: &str) -> syn::Result<T> {
    input.parse().map_err(|error| {
        syn::Error::new(
            error.span(),
            crate::args::takes_value("crud", Some(key), what),
        )
    })
}

impl Parse for CrudDeclaration {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut service = None;
        let mut entity = None;
        let mut output = None;
        let mut create = None;
        let mut update = None;
        let mut ops = OpsSelection::Default;
        let mut paginate = Paginate::Cursor;
        let mut paginate_written = None;
        let mut api = Vec::new();
        let mut api_written = None;

        CRUD.parse(input, |arg| {
            match arg.key() {
                "service" => {
                    service = Some(value_of(
                        arg.value()?,
                        "service",
                        "the name of the injected `CrudService` field, e.g. `service = svc`",
                    )?);
                }
                "entity" => {
                    entity = Some(value_of(
                        arg.value()?,
                        "entity",
                        "the entity's path, e.g. `entity = users::Entity`",
                    )?);
                }
                "output" => {
                    output = Some(value_of(
                        arg.value()?,
                        "output",
                        "the response type's path, e.g. `output = User`",
                    )?);
                }
                "create" => {
                    create = Some(value_of(
                        arg.value()?,
                        "create",
                        "the input type's path, e.g. `create = CreateUser`",
                    )?);
                }
                "update" => {
                    update = Some(value_of(
                        arg.value()?,
                        "update",
                        "the input type's path, e.g. `update = UpdateUser`",
                    )?);
                }
                "ops" => {
                    let ops_span = arg.ident().span();
                    let input = arg.value()?;
                    if !input.peek(syn::token::Bracket) {
                        return Err(syn::Error::new(
                            input.span(),
                            crate::args::takes_value("crud", Some("ops"), OPS_LIST),
                        ));
                    }
                    let content;
                    syn::bracketed!(content in input);
                    let idents = content.parse_terminated(
                        |element: ParseStream| value_of::<Ident>(element, "ops", OPS_LIST),
                        Token![,],
                    )?;
                    let mut selected = Vec::new();
                    for id in idents {
                        let Some(op) = op_named(&id.to_string()) else {
                            return Err(syn::Error::new(
                                id.span(),
                                crate::args::unknown_value(
                                    "crud",
                                    "op",
                                    &id.to_string(),
                                    CRUD_API.keys(),
                                ),
                            ));
                        };
                        selected.push(op);
                    }
                    if selected.is_empty() {
                        return Err(syn::Error::new(
                            ops_span,
                            format!(
                                "{} declares nothing — drop the argument to generate the \
                                 default set, or list the operations you want",
                                crate::args::site("crud", Some("ops = []")),
                            ),
                        ));
                    }
                    ops = OpsSelection::Explicit(selected, ops_span);
                }
                "paginate" => {
                    paginate_written = Some(arg.ident().span());
                    let mode: Ident = arg.value()?.parse().map_err(|error| {
                        syn::Error::new(
                            error.span(),
                            crate::args::takes_one_of("crud", "paginate", &PAGINATE),
                        )
                    })?;
                    paginate = match mode.to_string().as_str() {
                        "cursor" => Paginate::Cursor,
                        "none" => Paginate::None,
                        other => {
                            return Err(syn::Error::new(
                                mode.span(),
                                crate::args::unknown_value("crud", "paginate", other, &PAGINATE),
                            ));
                        }
                    };
                }
                "api" => {
                    api_written = Some(arg.ident().span());
                    api = parse_api(&arg)?;
                }
                // The grammar hands over only its own keys.
                _ => {}
            }
            Ok(())
        })?;

        let service = service.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                format!(
                    "{} (the injected `CrudService` field to delegate to)",
                    crate::args::missing_argument("crud", "service", "svc"),
                ),
            )
        })?;
        let entity = entity.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                crate::args::missing_argument("crud", "entity", "users::Entity"),
            )
        })?;
        let output = output.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                crate::args::missing_argument("crud", "output", "User"),
            )
        })?;

        Ok(CrudDeclaration {
            service,
            entity,
            output,
            create,
            update,
            ops,
            paginate,
            paginate_written,
            api,
            api_written,
        })
    }
}

/// Parse a `#[crud(...)]` attribute's tokens into a [`CrudDeclaration`].
pub fn parse_crud_args(args: TokenStream2) -> syn::Result<CrudDeclaration> {
    syn::parse2(args)
}

/// Snake-cased last segment of the output type (`ArtistExhibition` →
/// `artist_exhibition`); base for generated operation method names (the list op
/// is `<base>s`), which async-graphql camelCases.
///
/// Pluralization is naive (`Category` → `categorys`); hand-write the operation
/// when that matters — `#[crud]` skips any op a method of that name defines.
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

    fn parse(args: proc_macro2::TokenStream) -> syn::Result<CrudDeclaration> {
        parse_crud_args(args)
    }

    #[test]
    fn singular_of_snake_cases_compound_entity_names() {
        let compound: syn::Path = syn::parse_quote!(ArtistExhibition);
        assert_eq!(singular_of(&compound), "artist_exhibition");
        let single: syn::Path = syn::parse_quote!(User);
        assert_eq!(singular_of(&single), "user");
    }

    #[test]
    fn an_input_type_for_an_excluded_op_is_refused() {
        let cfg = parse(quote! {
            service = svc, entity = E, output = O, create = C, update = U, ops = [list, get]
        })
        .expect("the arguments parse");
        let Err(refusal) = cfg.generated_ops() else {
            panic!("an input type for an excluded op is refused");
        };
        let refusal = refusal.to_string();
        assert!(
            refusal.contains("`create`") && refusal.contains("an op `ops` leaves out"),
            "{refusal}",
        );
    }

    #[test]
    fn a_paginate_for_an_excluded_list_is_refused() {
        for mode in [quote!(none), quote!(cursor)] {
            let cfg = parse(quote! {
                service = svc, entity = E, output = O, ops = [get], paginate = #mode
            })
            .expect("the arguments parse");
            let Err(refusal) = cfg.generated_ops() else {
                panic!("a `paginate` for an excluded `list` is refused");
            };
            assert_eq!(
                refusal.to_string(),
                "#[crud] `paginate` configures the `list` op, which `ops` leaves out — list \
                 `list` in `ops` to generate it, or drop `paginate = …`",
            );
        }
        let reversed = parse(quote! {
            service = svc, entity = E, output = O, paginate = none, ops = [get, delete]
        })
        .expect("the arguments parse");
        assert!(reversed.generated_ops().is_err());
        let unwritten = parse(quote! { service = svc, entity = E, output = O, ops = [get] })
            .expect("the arguments parse");
        assert!(unwritten.generated_ops().is_ok());
        let listed = parse(quote! {
            service = svc, entity = E, output = O, ops = [list], paginate = none
        })
        .expect("the arguments parse");
        assert!(listed.generated_ops().is_ok_and(|ops| ops.list));
    }

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

    #[test]
    fn default_without_inputs_skips_create_and_update() {
        let cfg = parse(quote! { service = svc, entity = E, output = O }).expect("parses");
        let ops = cfg.generated_ops().expect("resolves");
        assert!(ops.list && ops.get && ops.delete);
        assert!(ops.create.is_none() && ops.update.is_none());
    }

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
