//! Shared parser for `#[crud(...)]`, consumed by the HTTP and GraphQL CRUD
//! generators. The grammar is the same on both surfaces; each generator reads
//! the fields it cares about (REST consumes `guards`; GraphQL ignores them).

use proc_macro2::{Span, TokenStream as TokenStream2};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, Path, Token};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Paginate {
    /// Keyset over the primary key. Default for new resources; free for
    /// UUID-v7 keys (ordered).
    Cursor,
    /// Offset (`page`/`per_page`). Random access at the cost of O(offset)
    /// scans and instability under concurrent inserts.
    Page,
}

pub struct CrudConfig {
    /// Field holding the entity's [`CrudService`] — every generated op
    /// delegates to it so controllers/resolvers never touch `Repo` directly.
    pub service: Ident,
    pub entity: Path,
    pub output: Path,
    pub create: Option<Path>,
    pub update: Option<Path>,
    /// Generate only `list` + `get`.
    pub readonly: bool,
    /// `None` returns the full (ability-scoped) collection.
    pub paginate: Option<Paginate>,
}

impl Parse for CrudConfig {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut service = None;
        let mut entity = None;
        let mut output = None;
        let mut create = None;
        let mut update = None;
        let mut readonly = false;
        let mut paginate = None;

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
                "readonly" => readonly = true,
                "paginate" => {
                    input.parse::<Token![=]>()?;
                    let mode: Ident = input.parse()?;
                    paginate = Some(match mode.to_string().as_str() {
                        "cursor" => Paginate::Cursor,
                        "page" => Paginate::Page,
                        _ => {
                            return Err(syn::Error::new(
                                mode.span(),
                                "expected `cursor` or `page`",
                            ));
                        }
                    });
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown #[crud] option `{other}` (expected `service`, `entity`, \
                             `output`, `create`, `update`, `readonly`, `paginate`)"
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
            readonly,
            paginate,
        })
    }
}

pub fn parse_crud_args(args: TokenStream2) -> syn::Result<CrudConfig> {
    syn::parse2(args)
}

/// Lowercased last segment of the output type (`User` → `user`); base for
/// generated operation names (the list op is `<singular>s`).
///
/// This is a naive lowercase, **not** real singularization/pluralization: an
/// irregular or already-plural entity yields an ungrammatical op name
/// (`Category` → list op `categorys`, `Person` → `persons`). When that matters,
/// hand-write the operation — `#[crud]` skips generating any op a method of the
/// same name already defines.
pub fn singular_of(output: &Path) -> String {
    output
        .segments
        .last()
        .map(|s| s.ident.to_string().to_lowercase())
        .unwrap_or_else(|| "item".to_owned())
}
