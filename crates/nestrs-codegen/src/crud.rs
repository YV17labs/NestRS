//! Shared parser for the `#[crud(...)]` attribute, consumed by both the HTTP
//! (`nestrs-http-macros`) and GraphQL (`nestrs-graphql-macros`) CRUD generators.
//!
//! `#[crud]` sits on a controller's or resolver's impl block and synthesises the
//! standard operations (list, get, create, update, delete) that the developer did
//! not hand-write, then re-emits the block under `#[routes]` / `#[resolver]`. The
//! grammar is the same on both surfaces; each generator reads the fields it needs
//! (REST consumes `guards`, GraphQL ignores them — its auth is the operation
//! bridge).

use proc_macro2::{Span, TokenStream as TokenStream2};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, Path, Token};

/// How a generated `list` paginates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Paginate {
    /// Keyset pagination over the primary key (the default for new resources;
    /// stable and index-friendly, and free for UUID-v7 keys, which are ordered).
    Cursor,
    /// Offset pagination (`page` / `per_page`) — random page access at the cost of
    /// O(offset) scans and instability under concurrent inserts.
    Page,
}

/// The parsed `#[crud(...)]` configuration.
pub struct CrudConfig {
    /// The injected field holding the entity's [`CrudService`] — every generated
    /// operation delegates to it (`service = users`), so the service stays the
    /// single ORM gateway. Controllers/resolvers never touch `Repo` directly.
    pub service: Ident,
    /// The SeaORM entity the CRUD operates on (`entity = users::entity::Entity`).
    pub entity: Path,
    /// The exposed output type returned to clients (`output = User`).
    pub output: Path,
    /// The create input type (`create = CreateUserInput`); omit to skip `create`.
    pub create: Option<Path>,
    /// The update input type (`update = UpdateUserInput`); omit to skip `update`.
    pub update: Option<Path>,
    /// Read-only: generate only `list` + `get`.
    pub readonly: bool,
    /// List pagination mode; `None` returns the full (ability-scoped) collection.
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
                            return Err(syn::Error::new(mode.span(), "expected `cursor` or `page`"))
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
                    ))
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

/// Parse a `#[crud(...)]` attribute's argument tokens into a [`CrudConfig`].
pub fn parse_crud_args(args: TokenStream2) -> syn::Result<CrudConfig> {
    syn::parse2(args)
}

/// The base name a generated resolver derives its operation names from: the
/// lowercased last segment of the output type (`User` → `user`), giving
/// `user`/`users`/`create_user`/`update_user`/`delete_user`.
pub fn singular_of(output: &Path) -> String {
    output
        .segments
        .last()
        .map(|s| s.ident.to_string().to_lowercase())
        .unwrap_or_else(|| "item".to_owned())
}
