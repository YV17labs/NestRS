//! Shared parser for `#[crud(...)]`, consumed by the HTTP and GraphQL CRUD
//! generators: each reads every key [`CrudDeclaration`] carries, and the types
//! its operations name off the service the `service` key points at
//! ([`CrudDeclaration::types`]).

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token, Type};

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
/// (`Create`/`Update`/`Delete`) each require the service to implement the
/// matching opt-in trait (`Creatable`/`Updatable`/`Deletable`), whose input
/// type the op reads: a missing impl is refused naming the trait and the op.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CrudOp {
    /// `GET /` — the collection, bounded by [`Paginate`].
    List,
    /// `GET /{id}` — one resource by primary key.
    Get,
    /// `POST /` — needs `Creatable`.
    Create,
    /// `PUT /{id}` — needs `Updatable`.
    Update,
    /// `DELETE /{id}` — needs `Deletable`.
    Delete,
}

impl CrudOp {
    /// The name `ops` and `api` give the op, and the method a controller
    /// writes to take it over.
    pub fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Get => "get",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    /// The method a resolver writes to take the op over, over the resource's
    /// [`resolver_singular`]: `users`, `user`, `create_user`, `update_user`,
    /// `delete_user`.
    pub fn resolver_method(self, singular: &str) -> String {
        match self {
            Self::List => format!("{singular}s"),
            Self::Get => singular.to_owned(),
            Self::Create => format!("create_{singular}"),
            Self::Update => format!("update_{singular}"),
            Self::Delete => format!("delete_{singular}"),
        }
    }
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
    /// No `ops = [...]` given: all five, each resolving only when the service
    /// implements its trait.
    Default,
    /// Explicit `ops = [...]`: exactly the listed ops. Carries the `ops` key
    /// span for diagnostics.
    Explicit(Vec<CrudOp>, Span),
}

/// Resolved per-op generation decision — the answer the generators consume:
/// an op selected and not written by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per operation `ops` names, read by both generators"
)]
pub struct GeneratedOps {
    /// Generate the collection read.
    pub list: bool,
    /// Generate the by-id read.
    pub get: bool,
    /// Generate the create.
    pub create: bool,
    /// Generate the update.
    pub update: bool,
    /// Generate the delete.
    pub delete: bool,
}

impl GeneratedOps {
    /// Whether any op is generated, so the block names the entity at all.
    pub fn any(self) -> bool {
        self.list || self.get || self.create || self.update || self.delete
    }

    /// Whether a generated op answers the entity's wire type.
    pub fn answers_rows(self) -> bool {
        self.list || self.get || self.create || self.update
    }
}

/// The types a `#[crud]` block's generated operations name, each a hidden
/// alias [`CrudDeclaration::types`] declares beside the block: read off the
/// service's `CrudService`, `Creatable` and `Updatable` impls and the entity's
/// `Exposed` one, and the function every generated operation reaches the
/// service through.
pub struct CrudTypes {
    /// The hidden function taking the host and answering the service its
    /// `#[inject]` field holds.
    pub service: Ident,
    /// The service's `CrudService::Entity`.
    pub entity: Ident,
    /// The entity's `Exposed::Wire`.
    pub output: Ident,
    /// The service's `Creatable::Create`.
    pub create: Ident,
    /// The service's `Updatable::Update`.
    pub update: Ident,
    /// The aliases and the function, declared beside the block.
    pub aliases: TokenStream2,
}

/// The parsed `#[crud(...)]` declaration both surface generators consume.
pub struct CrudDeclaration {
    /// The entity's `CrudService`, which every generated op delegates to,
    /// through the host's `#[inject]` field of that type, and reads its types
    /// off.
    pub service: Type,
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

    /// Resolve which ops to generate beside the block's own methods —
    /// `written` holds their names, `method_of` names the method that takes an
    /// op over on this edge. A selected op the block writes is not generated.
    ///
    /// Refused, never a silently dropped declaration: a key configuring an op
    /// `ops` leaves out, and `api(<op>(…))` or `paginate` beside the op written
    /// by hand, which nothing generated would read.
    pub fn generated_ops(
        &self,
        written: &HashSet<String>,
        method_of: impl Fn(CrudOp) -> String,
    ) -> syn::Result<GeneratedOps> {
        let selected = self.selected_ops()?;
        let by_hand = |op: CrudOp| {
            let method = method_of(op);
            written.contains(&method).then_some(method)
        };
        if let (Some(at), Some(method)) = (self.paginate_written, by_hand(CrudOp::List)) {
            return Err(hand_written_op_key(
                at,
                "paginate",
                CrudOp::List,
                &method,
                "drop `paginate = …`: the method bounds its own list",
            ));
        }
        for (op, _, at) in &self.api {
            if let Some(method) = by_hand(*op) {
                return Err(hand_written_op_key(
                    *at,
                    "api",
                    *op,
                    &method,
                    &format!("drop `{}(…)`: give that method its own `#[api]`", op.name()),
                ));
            }
        }
        Ok(GeneratedOps {
            list: selected.list && by_hand(CrudOp::List).is_none(),
            get: selected.get && by_hand(CrudOp::Get).is_none(),
            create: selected.create && by_hand(CrudOp::Create).is_none(),
            update: selected.update && by_hand(CrudOp::Update).is_none(),
            delete: selected.delete && by_hand(CrudOp::Delete).is_none(),
        })
    }

    /// The ops the declaration selects, refusing a key that configures an op
    /// `ops` leaves out.
    fn selected_ops(&self) -> syn::Result<GeneratedOps> {
        match &self.ops {
            OpsSelection::Default => Ok(GeneratedOps {
                list: true,
                get: true,
                create: true,
                update: true,
                delete: true,
            }),
            OpsSelection::Explicit(ops, _) => {
                let wants = |op| ops.contains(&op);
                if let (false, Some(written)) = (wants(CrudOp::List), self.paginate_written) {
                    return Err(excluded_op_key(written, "paginate", "list", "paginate = …"));
                }
                if let Some((op, _, written)) = self.api.iter().find(|(op, ..)| !wants(*op)) {
                    let name = op.name();
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
                    create: wants(CrudOp::Create),
                    update: wants(CrudOp::Update),
                    delete: wants(CrudOp::Delete),
                })
            }
        }
    }

    /// The types `ops` name on the block `#[crud]` decorates, `self_ty` its
    /// host named `host`: aliases of the projections off the service, and the
    /// one function reaching the service through the host's `#[inject]` field
    /// of its type, so a host injecting none is refused once, at the block.
    /// Only what a generated op reads is declared, so none is left unused.
    pub fn types(&self, host: &Ident, self_ty: &Type, ops: GeneratedOps) -> CrudTypes {
        let host = host.unraw();
        let alias = |what: &str| format_ident!("__NestrsCrud{}{}", host, what);
        let service = &self.service;
        let accessor = format_ident!(
            "__nestrs_crud_{}_service",
            crate::casing::snake_case(&host.to_string())
        );
        let entity = alias("Entity");
        let output = alias("Output");
        let create = alias("Create");
        let update = alias("Update");
        let reached = ops.any().then(|| {
            quote! {
                #[doc(hidden)]
                fn #accessor(host: &#self_ty) -> &#service {
                    <#self_ty as ::nest_rs_core::__private::Injects<#service>>::injected(host)
                }
                #[doc(hidden)]
                type #entity = <#service as ::nest_rs_seaorm::CrudService>::Entity;
            }
        });
        let output_alias = ops.answers_rows().then(|| {
            quote! {
                #[doc(hidden)]
                type #output = <#entity as ::nest_rs_seaorm::Exposed>::Wire;
            }
        });
        let create_alias = ops.create.then(|| {
            quote! {
                #[doc(hidden)]
                type #create = <#service as ::nest_rs_seaorm::Creatable>::Create;
            }
        });
        let update_alias = ops.update.then(|| {
            quote! {
                #[doc(hidden)]
                type #update = <#service as ::nest_rs_seaorm::Updatable>::Update;
            }
        });
        CrudTypes {
            service: accessor,
            entity,
            output,
            create,
            update,
            aliases: quote! { #reached #output_alias #create_alias #update_alias },
        }
    }
}

/// The refusal of a key that configures one op, declared beside an `ops` that
/// leaves that op out.
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

/// The refusal of a key that configures one op, declared beside the method
/// the block writes to take that op over: nothing generated would read it.
fn hand_written_op_key(
    written: Span,
    key: &str,
    op: CrudOp,
    method: &str,
    remedy: &str,
) -> syn::Error {
    syn::Error::new(
        written,
        format!(
            "{} configures the `{}` op, which this block writes by hand as `fn {method}` — \
             {remedy}",
            crate::args::site("crud", Some(key)),
            op.name(),
        ),
    )
}

/// Every key `#[crud]` takes, in declaration order.
const CRUD: Grammar = Grammar::new("crud", &["service", "ops", "paginate", "api"]);

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

const SERVICE: &str = "the type of the injected `CrudService`, e.g. `service = UsersService`";

/// `service`'s value: the service's type. A lone lowercase name reads as the
/// field holding it, the key's earlier spelling, and is refused naming what
/// to write.
fn service_type(input: ParseStream) -> syn::Result<Type> {
    let written: Type = value_of(input, "service", SERVICE)?;
    if let Type::Path(path) = &written
        && path.qself.is_none()
        && let Some(field) = path.path.get_ident()
        && field
            .unraw()
            .to_string()
            .starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
    {
        return Err(syn::Error::new_spanned(
            field,
            format!(
                "{} names the service's type, not the field holding it — write the type of \
                 `{field}`, e.g. `service = UsersService`: every generated operation reaches \
                 the service through the host's `#[inject]` field of that type",
                crate::args::site("crud", Some("service")),
            ),
        ));
    }
    Ok(written)
}

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
        let mut ops = OpsSelection::Default;
        let mut paginate = Paginate::Cursor;
        let mut paginate_written = None;
        let mut api = Vec::new();
        let mut api_written = None;

        CRUD.parse(input, |arg| {
            match arg.key() {
                "service" => service = Some(service_type(arg.value()?)?),
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
                    "{} (the injected `CrudService` every generated operation delegates to \
                     and reads its types off)",
                    crate::args::missing_argument("crud", "service", "UsersService"),
                ),
            )
        })?;

        Ok(CrudDeclaration {
            service,
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

/// The resource a `#[crud]` host serves, read off its name: its `role` suffix
/// dropped and the rest made [`singular`](crate::__private::singular) —
/// `UsersController` → `User`. A host named for its role alone keeps its
/// name.
pub fn resource_noun(host: &Ident, role: &str) -> String {
    let name = host.to_string();
    let stem = name
        .strip_suffix(role)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(&name);
    crate::casing::singular(stem)
}

/// The snake-cased resource a `#[crud]` resolver serves, the base of the
/// operation names it generates (`UsersResolver` → `user`, so `users`, `user`,
/// `create_user`, …), which async-graphql camelCases.
///
/// Pluralization is naive (`Category` → `categorys`), and so is the singular a
/// host named in the singular over a noun ending in `s` reads (`StatusResolver`
/// → `statu`, where `StatusesResolver` → `status`); hand-write the operation
/// when that matters — `#[crud]` skips any op a method of that name defines
/// ([`CrudOp::resolver_method`]).
pub fn resolver_singular(host: &Ident) -> String {
    crate::casing::snake_case(&resource_noun(host, "Resolver"))
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    fn parse(args: proc_macro2::TokenStream) -> syn::Result<CrudDeclaration> {
        parse_crud_args(args)
    }

    /// The ops a controller block writing none by hand generates.
    fn resolve(cfg: &CrudDeclaration) -> syn::Result<GeneratedOps> {
        cfg.generated_ops(&HashSet::new(), |op| op.name().to_owned())
    }

    const ALL: GeneratedOps = GeneratedOps {
        list: true,
        get: true,
        create: true,
        update: true,
        delete: true,
    };

    #[test]
    fn the_service_alone_generates_all_five_ops() {
        let cfg = parse(quote! { service = UsersService }).expect("the service alone parses");
        assert_eq!(resolve(&cfg).expect("resolves"), ALL);
    }

    #[test]
    fn the_keys_restating_the_services_types_are_refused() {
        for key in ["entity", "output", "create", "update"] {
            let ident = Ident::new(key, Span::call_site());
            let Err(refusal) = parse(quote! { service = UsersService, #ident = T }) else {
                panic!("`{key}` is no `#[crud]` key");
            };
            assert_eq!(
                refusal.to_string(),
                format!(
                    "unknown #[crud] argument `{key}`; expected `service`, `ops`, `paginate` or \
                     `api`"
                ),
            );
        }
    }

    #[test]
    fn a_crud_without_its_service_is_refused_naming_the_key() {
        let Err(refusal) = parse(quote! { ops = [list] }) else {
            panic!("the service is required");
        };
        assert!(refusal.to_string().contains("`service`"), "{refusal}");
    }

    #[test]
    fn the_types_are_read_off_the_service_the_key_names() {
        let cfg = parse(quote! { service = UsersService }).expect("parses");
        let host: Ident = parse_quote!(UsersController);
        let self_ty: Type = parse_quote!(UsersController);
        let types = cfg.types(&host, &self_ty, ALL);
        let aliases = types.aliases.to_string();
        assert!(
            aliases.contains(
                "fn __nestrs_crud_users_controller_service (host : & UsersController) -> & \
                 UsersService { < UsersController as :: nest_rs_core :: __private :: Injects < \
                 UsersService >> :: injected (host) }"
            ),
            "{aliases}",
        );
        for read in [
            "< UsersService as :: nest_rs_seaorm :: CrudService > :: Entity",
            "< __NestrsCrudUsersControllerEntity as :: nest_rs_seaorm :: Exposed > :: Wire",
            "< UsersService as :: nest_rs_seaorm :: Creatable > :: Create",
            "< UsersService as :: nest_rs_seaorm :: Updatable > :: Update",
        ] {
            assert!(aliases.contains(read), "{read}: {aliases}");
        }
        assert_eq!(
            types.entity.to_string(),
            "__NestrsCrudUsersControllerEntity"
        );
        assert_eq!(
            types.service.to_string(),
            "__nestrs_crud_users_controller_service"
        );
    }

    #[test]
    fn a_block_naming_its_host_by_path_reaches_its_service_by_type() {
        let cfg = parse(quote! { service = crate::users::UsersService }).expect("parses");
        let host: Ident = parse_quote!(UsersController);
        let self_ty: Type = parse_quote!(super::http::UsersController);
        let aliases = cfg.types(&host, &self_ty, ALL).aliases.to_string();
        assert!(
            aliases.contains(
                "< super :: http :: UsersController as :: nest_rs_core :: __private :: Injects < \
                 crate :: users :: UsersService >> :: injected (host)"
            ),
            "{aliases}",
        );
    }

    #[test]
    fn a_field_written_for_the_service_is_refused_naming_the_type_to_write() {
        for field in [quote!(svc), quote!(svc_users), quote!(r#notes)] {
            let Err(refusal) = parse(quote! { service = #field }) else {
                panic!("`{field}` reads as a field");
            };
            assert!(
                refusal.to_string().starts_with(
                    "#[crud] `service` names the service's type, not the field holding it"
                ),
                "{refusal}",
            );
        }
        assert!(parse(quote! { service = users::UsersService }).is_ok());
    }

    #[test]
    fn only_the_types_a_generated_op_reads_are_declared() {
        let cfg = parse(quote! { service = UsersService }).expect("parses");
        let host: Ident = parse_quote!(NotesController);
        let self_ty: Type = parse_quote!(NotesController);
        let read_only = GeneratedOps {
            create: false,
            update: false,
            delete: false,
            ..ALL
        };
        let aliases = cfg.types(&host, &self_ty, read_only).aliases.to_string();
        assert!(aliases.contains("Exposed"), "{aliases}");
        assert!(!aliases.contains("Creatable") && !aliases.contains("Updatable"));
        let delete_only = GeneratedOps {
            list: false,
            get: false,
            ..read_only
        };
        let delete_only = GeneratedOps {
            delete: true,
            ..delete_only
        };
        let aliases = cfg.types(&host, &self_ty, delete_only).aliases.to_string();
        assert!(aliases.contains("CrudService") && !aliases.contains("Exposed"));
        let none = GeneratedOps {
            delete: false,
            ..delete_only
        };
        assert!(cfg.types(&host, &self_ty, none).aliases.is_empty());
    }

    /// The names a GraphQL client asks for, pinned: a host named in the
    /// singular over a noun ending in `s` loses the `s`, and its plural reads
    /// the noun whole.
    #[test]
    fn a_singular_host_over_a_noun_ending_in_s_reads_it_without_the_s() {
        let names = |host: &str| {
            let singular = resolver_singular(&Ident::new(host, Span::call_site()));
            [CrudOp::Get, CrudOp::List, CrudOp::Create].map(|op| op.resolver_method(&singular))
        };
        assert_eq!(names("StatusResolver"), ["statu", "status", "create_statu"]);
        assert_eq!(
            names("StatusesResolver"),
            ["status", "statuss", "create_status"]
        );
        assert_eq!(names("AliasResolver"), ["alia", "alias", "create_alia"]);
        let controller = Ident::new("StatusController", Span::call_site());
        assert_eq!(resource_noun(&controller, "Controller"), "Statu");
    }

    #[test]
    fn the_resource_is_read_off_the_hosts_name() {
        for (host, role, noun) in [
            ("UsersController", "Controller", "User"),
            ("PostController", "Controller", "Post"),
            ("CategoriesResolver", "Resolver", "Category"),
            ("Things", "Controller", "Thing"),
            ("Controller", "Controller", "Controller"),
        ] {
            let host = Ident::new(host, Span::call_site());
            assert_eq!(resource_noun(&host, role), noun);
        }
        let host: Ident = parse_quote!(ArtistExhibitionsResolver);
        assert_eq!(resolver_singular(&host), "artist_exhibition");
    }

    #[test]
    fn a_key_beside_the_op_written_by_hand_is_refused_naming_the_method() {
        let written: HashSet<String> = ["users".to_owned(), "user".to_owned()].into();
        let resolver = |op: CrudOp| op.resolver_method("user");
        let paginated = parse(quote! { service = UsersService, paginate = none }).expect("parses");
        let Err(refusal) = paginated.generated_ops(&written, resolver) else {
            panic!("a `paginate` beside the hand-written list is refused");
        };
        assert_eq!(
            refusal.to_string(),
            "#[crud] `paginate` configures the `list` op, which this block writes by hand as \
             `fn users` — drop `paginate = …`: the method bounds its own list",
        );
        let documented = parse(quote! { service = UsersService, api(get(summary = "Get a user")) })
            .expect("parses");
        let Err(refusal) = documented.generated_ops(&written, resolver) else {
            panic!("an `api` for the hand-written op is refused");
        };
        assert!(
            refusal
                .to_string()
                .contains("`api` configures the `get` op")
                && refusal.to_string().contains("`fn user`"),
            "{refusal}",
        );
    }

    #[test]
    fn an_op_written_by_hand_is_not_generated() {
        let written: HashSet<String> = ["create".to_owned(), "delete".to_owned()].into();
        let cfg = parse(quote! { service = UsersService }).expect("parses");
        let ops = cfg
            .generated_ops(&written, |op| op.name().to_owned())
            .expect("the block's own methods resolve");
        assert!(ops.list && ops.get && ops.update);
        assert!(!ops.create && !ops.delete, "the block's own methods win");
    }

    #[test]
    fn each_op_is_named_as_ops_names_it_and_as_a_resolver_writes_it() {
        let ops = [
            CrudOp::List,
            CrudOp::Get,
            CrudOp::Create,
            CrudOp::Update,
            CrudOp::Delete,
        ];
        assert_eq!(
            ops.map(CrudOp::name),
            ["list", "get", "create", "update", "delete"]
        );
        assert_eq!(
            ops.map(|op| op.resolver_method("artist_exhibition")),
            [
                "artist_exhibitions",
                "artist_exhibition",
                "create_artist_exhibition",
                "update_artist_exhibition",
                "delete_artist_exhibition",
            ]
        );
        for op in ops {
            assert_eq!(op_named(op.name()), Some(op), "{op:?}");
        }
    }

    #[test]
    fn a_paginate_for_an_excluded_list_is_refused() {
        for mode in [quote!(none), quote!(cursor)] {
            let cfg = parse(quote! { service = UsersService, ops = [get], paginate = #mode })
                .expect("the arguments parse");
            let Err(refusal) = resolve(&cfg) else {
                panic!("a `paginate` for an excluded `list` is refused");
            };
            assert_eq!(
                refusal.to_string(),
                "#[crud] `paginate` configures the `list` op, which `ops` leaves out — list \
                 `list` in `ops` to generate it, or drop `paginate = …`",
            );
        }
        let reversed =
            parse(quote! { service = UsersService, paginate = none, ops = [get, delete] })
                .expect("the arguments parse");
        assert!(resolve(&reversed).is_err());
        let unwritten = parse(quote! { service = UsersService, ops = [get] }).expect("parses");
        assert!(resolve(&unwritten).is_ok());
        let listed = parse(quote! { service = UsersService, ops = [list], paginate = none })
            .expect("parses");
        assert!(resolve(&listed).is_ok_and(|ops| ops.list));
    }

    #[test]
    fn explicit_partial_selection_generates_only_listed_ops() {
        let cfg =
            parse(quote! { service = UsersService, ops = [list, get, delete] }).expect("parses");
        let ops = resolve(&cfg).expect("resolves");
        assert!(ops.list && ops.get && ops.delete);
        assert!(!ops.create && !ops.update);
        let cfg = parse(quote! { service = UsersService, ops = [get, create] }).expect("parses");
        let ops = resolve(&cfg).expect("resolves");
        assert!(ops.get && ops.create);
        assert!(!ops.list && !ops.update && !ops.delete);
    }

    #[test]
    fn unknown_op_name_is_rejected() {
        let err = match parse(quote! { service = UsersService, ops = [list, frobnicate] }) {
            Ok(_) => panic!("unknown op must fail to parse"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("frobnicate"));
    }
}
