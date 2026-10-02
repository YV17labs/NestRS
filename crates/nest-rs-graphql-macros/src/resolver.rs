//! `#[resolver]`: construction on the struct. `#[operations]`: the
//! orchestration of the `#[query]` / `#[mutation]` / `#[subscription]` /
//! `#[field_resolver]` methods on its impl block.
//!
//! Two decorators rather than one accepting two item shapes, because an
//! attribute macro is a single path in the macro namespace: the shape is
//! discriminated *after* `syn::parse`, so a shared name gives one rustdoc page
//! for two argument grammars and annotates every expansion error with the same
//! attribute whichever half emitted it. See the *one decorator, one item shape*
//! rule in `CLAUDE.md`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, FnArg, GenericArgument, Ident, ImplItem, ItemImpl, ItemStruct, LitStr, Path,
    PathArguments, Signature, Token, Type, parse_quote,
};

use nest_rs_codegen::{
    Collision, Conditional, DecoratorPair, DispatchKeys, Edge, HostBorrow, InjectableBody,
    PipeWrapper, await_if_async, build_injectable_body, cfg_attrs, delegated_attrs,
    force_guard_typeids, forwarded_arg_idents, forwarded_idents, from_container_method,
    guard_capability_bounds, impl_self_ident, injected_keys_with_layers,
    injected_methods_with_layers, injected_names_with_layers, layer_deps, normalize_forwarded_args,
    pipe_wrapper, reject_http_only_layers, scoped_specs, shared_receiver, take_flag_attr,
    take_path_list,
};

/// The GraphQL edge's pair, read by `#[resolver]`, `#[operations]` and `#[crud]`.
pub(crate) const GRAPHQL_PAIR: DecoratorPair = DecoratorPair {
    host: "#[resolver]",
    subject: "resolver struct",
    operations: "#[operations]",
    collects: "#[query] / #[mutation] / #[subscription] / #[entity] / #[field_resolver]",
};

pub(crate) fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    // `version = "…"` before the blanket refusal below: the developer arriving
    // from `#[controller(version = "1")]` asked a real question, and "takes no
    // arguments" answers a different one. Only the host half carries this — the
    // sentence names `#[resolver]`, which is where a version would be declared
    // if the schema had one. The wording is `nest-rs-codegen`'s, so GraphQL's
    // answer lives where every edge's does.
    if let Err(err) = Edge::Graphql.reject_version(&args) {
        return err.to_compile_error().into();
    }
    if let Err(err) = reject_resolver_args(&args) {
        return err.to_compile_error().into();
    }

    // Naming the sibling is the whole point of the split: the shape a developer
    // reached for exists, it is just spelled with the other decorator. Both
    // halves read `GRAPHQL_PAIR`, so the two sentences cannot drift.
    match GRAPHQL_PAIR.parse_host(input.into()) {
        Ok(item) => resolver_struct(item),
        Err(err) => err.to_compile_error().into(),
    }
}

pub(crate) fn operations(args: TokenStream, input: TokenStream) -> TokenStream {
    // The shared sentence, from the same pair the wrong-shape error reads: the
    // operation set it names is `GRAPHQL_PAIR.collects`, so adding a role — this
    // is how `#[entity]` arrived — cannot leave one of the two listing the old
    // set.
    //
    // Refused through the kept item like every other refusal of this half: an
    // early return dropped the whole `impl`, so the one real error arrived under
    // `no method found` at every caller and `Discoverable` at the module.
    let written = TokenStream2::from(input.clone());
    let expansion: TokenStream = match GRAPHQL_PAIR
        .reject_args(
            &TokenStream2::from(args),
            "a resolver's construction and provider-scope layers are declared by",
        )
        .and_then(|()| GRAPHQL_PAIR.parse_operations(input.into()))
    {
        Ok(item) => resolver_impl(item),
        Err(err) => err.to_compile_error().into(),
    };
    GRAPHQL_PAIR
        .keep_item_on_refusal(written, expansion.into(), &OPERATIONS_HELPERS, |item| {
            let self_ty = &item.self_ty;
            quote! {
                impl ::nest_rs_core::Discoverable for #self_ty {
                    fn register(
                        builder: ::nest_rs_core::ContainerBuilder,
                    ) -> ::nest_rs_core::ContainerBuilder {
                        builder
                    }
                }
            }
        })
        .into()
}

/// What `#[operations]` consumes off a method beside the layers and the
/// posture: the roles, and async-graphql's own helper, which only the delegate
/// it hands the method to can read.
const OPERATIONS_HELPERS: [&str; 6] = [
    "query",
    "mutation",
    "subscription",
    "entity",
    "field_resolver",
    "graphql",
];

/// The **host** half takes no arguments either, which no other edge's does:
/// `#[controller]` and `#[gateway]` declare a path, `#[mcp]` an endpoint. A
/// resolver has no address to declare — one schema, one introspection — so the
/// sentence points at the operations instead of at a sibling argument.
///
/// One site, so it stays here rather than on `DecoratorPair`; it reads the
/// pair's own `collects` all the same, so it and the impl half's refusal cannot
/// come to name different operation sets.
fn reject_resolver_args(args: &TokenStream2) -> syn::Result<()> {
    if args.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new_spanned(
        args,
        format!(
            "{} takes no arguments; tag methods with {} under {}",
            GRAPHQL_PAIR.host, GRAPHQL_PAIR.collects, GRAPHQL_PAIR.operations
        ),
    ))
}

/// `#[resolver]` on the struct: construction + provider-scope layer
/// declarations (parallel to `#[controller]` on the struct, `#[gateway]` on
/// the struct). The impl-form macro reads the layer specs back at runtime
/// via the inherent `__nestrs_resolver_*_specs()` helpers emitted here.
fn resolver_struct(mut item: ItemStruct) -> TokenStream {
    if let Err(err) = reject_http_only_layers(&item.attrs, "GraphQL", "resolver") {
        return err.to_compile_error().into();
    }
    // Resolver-scope (provider) guard declarations — same shape and same
    // mental model as `#[controller] struct` + `#[gateway] struct`. Stored
    // here so the impl-form macro can fold them into the per-operation
    // chain at runtime through `__nestrs_resolver_guard_specs()`.
    let guards = match take_use_guards(&mut item.attrs) {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        ..
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let name_str = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The struct's `#[inject]` keys + any resolver-scope guards, exposed
    // for the impl-block macro to fold into `Discoverable::injected`
    // together with method guards and `#[field_resolver]` `&Service`
    // deps. Same struct/impl split as `#[controller]`/`#[routes]`.
    // Keys plus index-aligned labels from one walk, so a resolver-scope guard no
    // module provides is named in the boot error rather than reported as
    // `<unnamed dependency>`.
    let layers = layer_deps(guards.iter());
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    // Resolver-scope guards fold into the same per-operation chain, so they owe
    // the same capability.
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::GraphqlGuard));

    // Resolver-membership marker so the boot can require this resolver be
    // listed in a reachable module's `providers` (its schema presence is
    // unconditional via the registry). A generic resolver has no single
    // `TypeId` so it can't be a `providers` entry.
    let descriptor = if item.generics.params.is_empty() {
        quote! {
            ::nest_rs_graphql::inventory::submit! {
                ::nest_rs_graphql::ResolverDescriptor {
                    resolver: || ::core::any::TypeId::of::<#name>(),
                    name: #name_str,
                }
            }
        }
    } else {
        quote!()
    };

    let residency = GRAPHQL_PAIR.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
            }

            #[doc(hidden)]
            pub fn __nestrs_injected_names() -> ::std::vec::Vec<&'static str> {
                #injected_names
            }

            /// Resolver-scope `#[use_guards(...)]`, exposed for the
            /// impl-form macro to fold into each operation's per-chain
            /// `run_layered_graphql_chain` call. Empty when none declared.
            #[doc(hidden)]
            pub fn __nestrs_resolver_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                #guard_specs
            }
        }

        #descriptor
    }
    .into()
}

/// Extract and remove a `#[use_guards(...)]` attribute, returning its paths.
/// The attribute is consumed so it never reaches the compiler as an unknown
/// attribute. At most one per item.
fn take_use_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    take_path_list(attrs, "use_guards")
}

/// `#[force_guards(...)]` — the Layer-System opt-in that lets a per-method
/// guard re-run even when the same `TypeId` is already in the global chain.
/// Same shape as `#[use_guards]`.
fn take_force_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    take_path_list(attrs, "force_guards")
}

/// `#[authorize(Action, Entity)]` parsed off a `#[query]`/`#[mutation]`
/// method: the operation's declared access posture. The macro emits the
/// class-level gate (`authorize::<Action, Entity>`) before the call and the
/// automatic response mask (`masked_value_for`) after it — the GraphQL analog
/// of the HTTP `Authorize<A, E>` extractor. `unmasked` keeps the gate but
/// leaves response masking to the method body (custom shapes the value-level
/// round-trip cannot see through, e.g. a cursor connection).
struct AuthorizeSpec {
    action: Path,
    /// The entity the gate + mask act on. Explicit (`#[authorize(Action,
    /// Entity)]`) or, when `bind = Service` is set, **derived** from
    /// `<Service as CrudService>::Entity` so it is never retyped —
    /// `#[authorize(Update, bind = ArtworksService)]`.
    entity: Option<Path>,
    unmasked: bool,
    /// `bind = Service`: the macro turns a by-id GraphQL argument into the
    /// loaded, authorized subject and hands it to the operation's
    /// `Authorized<Action, E>` parameter — the GraphQL analog of the HTTP
    /// `Bind<A, S>` extractor. The action in the proof is the one named here, so
    /// the receiving method demands a proof for *exactly* that action. `None` ⇒
    /// the operation binds its subject itself (or has none).
    bind: Option<Path>,
    /// The wire name of the synthesized id argument when `bind` is set, as a
    /// snake_case ident (async-graphql camelCases it). `None` defaults to `id`;
    /// `id_arg = file_id` yields `fileId` to preserve an existing SDL argument.
    id_arg: Option<Ident>,
}

/// One token in `#[authorize(...)]`: a positional `Path` (action, entity, or
/// the `unmasked` flag) or a `name = value` option (`bind = Service`,
/// `id_arg = ident`).
enum AuthorizeArg {
    Positional(Path),
    /// `bind = Service`, with the key as written — what a repeat is spanned at.
    Bind(Ident, Path),
    /// `id_arg = ident`, with the key as written.
    IdArg(Ident, Ident),
}

/// The keys `#[authorize]` takes beside its positionals.
const AUTHORIZE_KEYS: [&str; 2] = ["bind", "id_arg"];

impl syn::parse::Parse for AuthorizeArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        // Each value refused at the token syn stopped on, in a sentence naming
        // the decorator and the key — or, for a positional, in the attribute's
        // own shape sentence, which is what the other three edges answer it with.
        if input.peek(Ident) && input.peek2(Token![=]) {
            let name: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            if name == "bind" {
                input
                    .parse()
                    .map(|p| AuthorizeArg::Bind(name, p))
                    .map_err(refused(
                        "bind",
                        "the path of the service that loads the subject, e.g. \
                     `bind = FilesService`",
                    ))
            } else if name == "id_arg" {
                input.parse().map(|i| AuthorizeArg::IdArg(name, i)).map_err(refused(
                    "id_arg",
                    "the id argument's name as a snake_case identifier, e.g. `id_arg = file_id`",
                ))
            } else {
                let spelled = name.to_string();
                Err(syn::Error::new_spanned(
                    name,
                    nest_rs_codegen::unknown_argument("authorize", &spelled, &AUTHORIZE_KEYS),
                ))
            }
        } else {
            input
                .parse()
                .map(AuthorizeArg::Positional)
                .map_err(|stopped| syn::Error::new(stopped.span(), AUTHORIZE_SHAPE))
        }
    }
}

/// A keyed `#[authorize]` value of the wrong kind, re-worded at the token syn
/// stopped on: the shared value sentence, naming the decorator and the key.
fn refused(key: &'static str, takes: &'static str) -> impl Fn(syn::Error) -> syn::Error {
    move |stopped| {
        syn::Error::new(
            stopped.span(),
            nest_rs_codegen::takes_value("authorize", Some(key), takes),
        )
    }
}

/// The shape of `#[authorize(...)]` on an operation, quoted whenever what was
/// written is not it — a wrong number of positionals, or a positional that is
/// not a path.
const AUTHORIZE_SHAPE: &str = "expected `#[authorize(Action, Entity)]` — e.g. \
     `#[authorize(Read, users::Entity)]`; append `unmasked` to keep the class gate but mask the \
     response yourself. `bind = Service` (optionally `id_arg = ident`) binds the subject from an \
     id argument, and lets the entity be omitted (derived from `Service::Entity`): \
     `#[authorize(Update, bind = ArtworksService)]`";

/// Extract and remove a `#[authorize(...)]` attribute. At most one per method.
fn take_authorize(attrs: &mut Vec<Attribute>) -> syn::Result<Option<AuthorizeSpec>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("authorize")) else {
        return Ok(None);
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident("authorize")) {
        return Err(syn::Error::new_spanned(
            &attr,
            nest_rs_codegen::at_most_one_authorize("operation"),
        ));
    }
    let args: Vec<AuthorizeArg> = attr
        .parse_args_with(Punctuated::<AuthorizeArg, Token![,]>::parse_terminated)?
        .into_iter()
        .collect();
    let shape_err = || syn::Error::new_spanned(&attr, AUTHORIZE_SHAPE);
    let mut positional: Vec<Path> = Vec::new();
    let mut bind: Option<Path> = None;
    let mut id_arg: Option<Ident> = None;
    // Refused rather than last-write-wins: `bind` decides **which service loads
    // the authorized subject**, so dropping one of two by source order is the
    // posture silently deciding itself.
    let mut written = nest_rs_codegen::WrittenKeys::default();
    for arg in args {
        match arg {
            AuthorizeArg::Positional(p) => positional.push(p),
            AuthorizeArg::Bind(key, p) => {
                written.take_key("authorize", &AUTHORIZE_KEYS, &key, &key.to_string())?;
                bind = Some(p);
            }
            AuthorizeArg::IdArg(key, i) => {
                written.take_key("authorize", &AUTHORIZE_KEYS, &key, &key.to_string())?;
                id_arg = Some(i);
            }
        }
    }
    if id_arg.is_some() && bind.is_none() {
        return Err(syn::Error::new_spanned(
            &attr,
            "`id_arg` only applies with `bind = Service`",
        ));
    }
    let unmasked = positional.iter().any(|p| p.is_ident("unmasked"));
    let mut subject: Vec<Path> = positional
        .into_iter()
        .filter(|p| !p.is_ident("unmasked"))
        .collect();
    // `Action, Entity` always; `Action` alone is allowed only with `bind`,
    // where the entity is derived from `Service::Entity` (never retyped).
    let (action, entity) = match (subject.len(), bind.is_some()) {
        (2, _) => {
            let entity = subject.remove(1);
            (subject.remove(0), Some(entity))
        }
        (1, true) => (subject.remove(0), None),
        _ => return Err(shape_err()),
    };
    Ok(Some(AuthorizeSpec {
        action,
        entity,
        unmasked,
        bind,
        id_arg,
    }))
}

/// The ident of a `#[query]`/`#[mutation]` parameter typed `Authorized<A, E>`
/// (the subject `bind = Service` resolves). Matched on the last path segment so
/// both `Authorized<A, E>` and a fully-qualified form are recognised.
fn authorized_param_ident(sig: &Signature) -> Option<Ident> {
    sig.inputs.iter().find_map(|arg| {
        let FnArg::Typed(pt) = arg else { return None };
        let Type::Path(tp) = &*pt.ty else { return None };
        if tp.path.segments.last()?.ident != "Authorized" {
            return None;
        }
        match &*pt.pat {
            syn::Pat::Ident(pi) => Some(pi.ident.clone()),
            _ => None,
        }
    })
}

/// Grouping key for a `#[field_resolver]`'s parent type — its **last path
/// segment**. Two spellings of one type (`User` and `crate::wire::User`) share
/// a last segment, so their field resolvers merge into a single
/// `#[ComplexObject]` block instead of splitting into two impls that then
/// collide as an opaque `E0119` duplicate-impl error. Mirrors the last-segment
/// matching in [`authorized_param_ident`]. A non-path type (rare for a wire
/// parent) falls back to its full token string.
fn field_parent_key(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
            .unwrap_or_else(|| quote!(#ty).to_string()),
        _ => quote!(#ty).to_string(),
    }
}

/// A `#[query]`/`#[mutation]` parameter typed `Piped<P, T>` or `Valid<T>` — a
/// per-argument pipe. The wrapper exposes the wire value type `T` in the
/// parameter's place, runs the pipe (`P::transform` / validation), and hands the
/// operation the `Piped`/`Valid` carrier — the GraphQL analog of the HTTP
/// `Piped<P, E>` / `Valid<E>` extractors. A pipe transforms input only; it never
/// decides authz (that stays the `#[authorize]` gate's job).
struct PipedArg {
    ident: Ident,
    /// The pipe `P` in `Piped<P, T>`; `None` for `Valid<T>` (validation).
    pipe: Option<Path>,
    /// The wire value type `T` the operation exposes and the pipe consumes.
    value_ty: Type,
}

/// Every `Piped<P, T>` / `Valid<T>` parameter of an operation, matched on the
/// last path segment so a fully-qualified form is recognised too.
fn piped_args(sig: &Signature) -> Vec<PipedArg> {
    sig.inputs
        .iter()
        .filter_map(|arg| {
            let FnArg::Typed(pt) = arg else { return None };
            let syn::Pat::Ident(pi) = &*pt.pat else {
                return None;
            };
            let (pipe, value_ty) = match pipe_wrapper(&pt.ty)? {
                PipeWrapper::Piped { pipe, value } => (Some(pipe), value),
                PipeWrapper::Valid { value } => (None, value),
            };
            Some(PipedArg {
                ident: pi.ident.clone(),
                pipe,
                value_ty,
            })
        })
        .collect()
}

/// What a method's return type is, read the way async-graphql reads it.
///
/// **Fallibility is the spelling async-graphql's own derive reads, and nothing
/// else**: a return is fallible when its last path segment is exactly `Result`
/// or `FieldResult` and carries a type argument — the first one is the value
/// (`async-graphql-derive`'s `OutputType::parse`). The wrapper is handed to that
/// derive, so mirroring its rule is the one reading that cannot disagree with
/// the registry. The rule this replaced asked whether the segment *ended* with
/// `Result`, which took `SearchResult` — an ordinary `SimpleObject` — for a
/// `Result`, emitted the developer's type as the wrapper's and left three rustc
/// errors on `#[operations]`.
///
/// **What the spelling cannot see, the type checker does.** A `Result` under
/// another name — `use async_graphql::Result as GqlResult`, a type alias —
/// reads as a value here, and every value answer goes through
/// `nest_rs_core::Answer` ([`call_as_result`]), which tells a `Result` by its
/// type and splits its error into the wrapper's.
enum Returned<'a> {
    /// Spelled fallible; the value type is the first type argument.
    Fallible(&'a Type),
    /// Anything else — `()` for a method that declares no return type. Boxed
    /// beside its borrowed sibling, which is a pointer.
    Value(Box<Type>),
}

impl Returned<'_> {
    /// The value the wrapper answers with, `T` of its `async_graphql::Result<T>`.
    fn value(&self) -> Type {
        match self {
            Self::Fallible(ty) => (*ty).clone(),
            Self::Value(ty) => (**ty).clone(),
        }
    }
}

/// Whether `ty` holds an `impl Trait` anywhere but as the whole of itself — in a
/// path's arguments, behind a reference, in a tuple or an array.
fn nests_impl_trait(ty: &Type) -> bool {
    fn inside(ty: &Type) -> bool {
        match ty {
            Type::ImplTrait(_) => true,
            Type::Path(tp) => tp.path.segments.iter().any(|segment| {
                matches!(&segment.arguments, PathArguments::AngleBracketed(args)
                    if args.args.iter().any(|arg| matches!(arg, GenericArgument::Type(ty) if inside(ty))))
            }),
            Type::Reference(r) => inside(&r.elem),
            Type::Paren(p) => inside(&p.elem),
            Type::Group(g) => inside(&g.elem),
            Type::Tuple(t) => t.elems.iter().any(inside),
            Type::Array(a) => inside(&a.elem),
            Type::Slice(sl) => inside(&sl.elem),
            _ => false,
        }
    }
    !matches!(ty, Type::ImplTrait(_)) && inside(ty)
}

fn returned(sig: &Signature) -> Returned<'_> {
    let ty = match &sig.output {
        syn::ReturnType::Default => return Returned::Value(Box::new(parse_quote!(()))),
        syn::ReturnType::Type(_, ty) => &**ty,
    };
    if let Type::Path(tp) = ty
        && let Some(last) = tp.path.segments.last()
        && (last.ident == "Result" || last.ident == "FieldResult")
        && let PathArguments::AngleBracketed(args) = &last.arguments
        && let Some(value) = args.args.iter().find_map(|arg| match arg {
            GenericArgument::Type(value) => Some(value),
            _ => None,
        })
    {
        return Returned::Fallible(value);
    }
    Returned::Value(Box::new(ty.clone()))
}

/// The ident of a method's `&Context<'_>` parameter (matched on the last
/// path segment), so guard injection reuses it instead of adding a second.
pub(crate) fn ctx_param_ident(sig: &Signature) -> Option<Ident> {
    sig.inputs.iter().find_map(ctx_ident_of)
}

/// Whether one parameter is a `&Context<'_>`, and what it is called.
fn ctx_ident_of(arg: &FnArg) -> Option<Ident> {
    let FnArg::Typed(pt) = arg else { return None };
    let Type::Reference(reference) = &*pt.ty else {
        return None;
    };
    let Type::Path(tp) = &*reference.elem else {
        return None;
    };
    if tp.path.segments.last()?.ident != "Context" {
        return None;
    }
    match &*pt.pat {
        syn::Pat::Ident(pi) => Some(pi.ident.clone()),
        _ => None,
    }
}

/// Refuse a `&Context` that is not the operation's **first** parameter after
/// the receiver.
///
/// async-graphql's `#[Object]` recognises the context parameter only there and
/// reads a later one as a schema argument — so what a misplaced one produces is
/// an `InputType` bound failure against `&ContextBase<…>`, named nowhere in the
/// developer's source. On an `#[entity]` it is worse than unreadable: the stray
/// parameter joins the `@key` a router matches references against, silently
/// changing what the operation is addressed by.
///
/// [`ensure_ctx_param`] already knew the rule — it inserts at position 1 — but
/// only handled the *absent* case: finding a `&Context` anywhere made it decline
/// to insert one, and the misplaced one then went to async-graphql as an
/// argument.
fn reject_misplaced_ctx(sig: &Signature) -> syn::Result<()> {
    let expected = usize::from(matches!(sig.inputs.first(), Some(FnArg::Receiver(_))));
    for (index, arg) in sig.inputs.iter().enumerate() {
        if index == expected || ctx_ident_of(arg).is_none() {
            continue;
        }
        return Err(syn::Error::new_spanned(
            arg,
            "a `&Context` parameter comes first, directly after `&self` — async-graphql\'s \
             `#[Object]` recognises it only there and reads a later one as a schema argument, \
             which fails as an `InputType` bound on a type you never wrote. On an `#[entity]` \
             it also joins the `@key` the router matches on. Move it up, or drop it: the \
             decorator inserts one when the operation needs it",
        ));
    }
    Ok(())
}

/// Ensure the delegating signature has a `&Context`. async-graphql's
/// `#[Object]` recognises the context parameter **only directly after
/// `&self`** (any later `&Context` is read as a schema argument), so the
/// added parameter is inserted at position 1.
fn ensure_ctx_param(sig: &Signature) -> (Signature, Ident) {
    if let Some(ident) = ctx_param_ident(sig) {
        return (sig.clone(), ident);
    }
    let ident = format_ident!("__guard_ctx");
    let mut sig = sig.clone();
    sig.inputs.insert(
        1,
        parse_quote!(#ident: &::nest_rs_graphql::async_graphql::Context<'_>),
    );
    (sig, ident)
}

/// Which `::nest_rs_guards::GraphqlSite` a chain names — the one thing the
/// roles' chains differ by.
#[derive(Clone, Copy)]
enum ChainSite {
    /// A root field: the app-wide pool folds in.
    Operation,
    /// An `#[entity]`: the federation gate in front of `_entities` runs the pool.
    Entity,
    /// A `#[field_resolver]`: the root field it resolves under ran the pool.
    Field,
}

impl ChainSite {
    fn path(self) -> TokenStream2 {
        match self {
            Self::Operation => quote!(::nest_rs_guards::GraphqlSite::Operation),
            Self::Entity => quote!(::nest_rs_guards::GraphqlSite::Entity),
            Self::Field => quote!(::nest_rs_guards::GraphqlSite::Field),
        }
    }
}

/// Emit the unified Layer System chain for a resolver operation: global +
/// resolver-scope + per-method guards, deduped by `TypeId`. Resolver-scope
/// guards are read at runtime via `<Self>::__nestrs_resolver_guard_specs()`
/// — emitted by `#[resolver]` on the struct, parallel to how
/// `#[controller]` exposes `__nestrs_controller_guard_specs()` for
/// `#[routes]` to consume. This is what makes the declaration site uniform:
/// the developer writes `#[use_guards(...)]` on the struct, same as for
/// HTTP controllers and WS gateways.
///
/// **Always emitted, whatever the operation returns.** It was once left out of
/// an operation returning a bare `T`, on the reasoning that a bare body has
/// nowhere to put a denial — so a resolver-scope or app-wide guard protected
/// only the operations that happened to return `Result`, and `-> Vec<Secret>`
/// under a deny-all resolver guard served the data. The failure channel is the
/// *wrapper's*: every method the expansion emits returns `Result`
/// ([`wrapper_output`]), so the chain's `?` always has somewhere to go and a
/// denial is a field error whatever the developer's method returns.
///
/// `site` names the pool's treatment rather than picking a function: the
/// runner is one seam.
fn layered_resolver_chain(
    self_ty: &Type,
    method_guards: &[Path],
    force_guards: &[Path],
    ctx: &Ident,
    route_label: &str,
    site: ChainSite,
) -> TokenStream2 {
    let label_lit = LitStr::new(route_label, proc_macro2::Span::call_site());
    let method_specs = scoped_specs(method_guards, quote!(dyn ::nest_rs_guards::Guard));
    let force_typeids = force_guard_typeids(force_guards);
    let site = site.path();
    quote! {
        {
            // Composed once per site against this container, then memoized —
            // the GraphQL analog of `RouteShaper`'s mount-time composition.
            static __NESTRS_GUARD_CHAIN: ::nest_rs_guards::SiteChainCell =
                ::nest_rs_guards::SiteChainCell::new();
            let __container = #ctx.data_unchecked::<::nest_rs_core::Container>();
            ::nest_rs_guards::run_layered_graphql_chain(
                #ctx,
                __container,
                &__NESTRS_GUARD_CHAIN,
                #label_lit,
                &|| ::nest_rs_guards::SiteChainSources {
                    provider: <#self_ty>::__nestrs_resolver_guard_specs(),
                    method: #method_specs,
                    force: #force_typeids,
                },
                #site,
            ).await?;
        }
    }
}

/// The return type of the method the expansion emits: always
/// `async_graphql::Result<T>`, `T` the value [`returned`] reads.
///
/// One shape whatever the developer wrote, so every chain step has a failure
/// channel and the developer's own error type owes nothing to it: the wrapper
/// returning the developer's `Result<T, MyError>` made every `?` in the chain
/// require `MyError: From<async_graphql::Error>`, a bound no page stated.
fn wrapper_output(sig: &Signature) -> syn::ReturnType {
    let value = returned(sig).value();
    parse_quote!(-> ::nest_rs_graphql::async_graphql::Result<#value>)
}

/// The developer's call as the wrapper's `async_graphql::Result<T>` — the value
/// the wrapper's posture, mask and operation line all read.
///
/// A fallible return converts its error the way async-graphql would have
/// (`Into<async_graphql::Error>`). A value return goes through
/// `nest_rs_core::Answer`, which knows a `Result` by its type: one under another
/// name has its error split into the wrapper's, so a failure files
/// `error` and reaches the client as one, whatever the type is called. A
/// `#[subscription]` cannot be answered that way — async-graphql's derive takes
/// any value for the stream itself — so there the probe refuses it at compile
/// time, at the return type, with the fix. Zero-sized and resolved by the
/// compiler: the request pays nothing for either.
fn call_as_result(sig: &Signature, call: TokenStream2, root: RootKind) -> TokenStream2 {
    if let Returned::Fallible(_) = returned(sig) {
        return quote! {
            ::core::result::Result::map_err(
                #call,
                ::core::convert::Into::<::nest_rs_graphql::async_graphql::Error>::into,
            )
        };
    }
    if root == RootKind::Subscription {
        let span = match &sig.output {
            syn::ReturnType::Type(_, ty) => ty.span(),
            syn::ReturnType::Default => sig.ident.span(),
        };
        let probe = quote_spanned! {span=>
            ::nest_rs_graphql::answers_a_stream(::nest_rs_core::Answer(&__answer).kind());
        };
        return quote! {{
            use ::nest_rs_core::AnswerFallback as _;
            let __answer = #call;
            #probe
            ::core::result::Result::<_, ::nest_rs_graphql::async_graphql::Error>::Ok(__answer)
        }};
    }
    quote! {{
        use ::nest_rs_core::AnswerFallback as _;
        let __answer = #call;
        ::nest_rs_core::Answer(&__answer).split::<::nest_rs_graphql::async_graphql::Error>()(
            __answer,
        )
    }}
}

/// The closed role vocabulary, read by the verb predicate **and** by the
/// one-role refusal — so the set a method is checked against and the set it is
/// told about cannot disagree.
const ROLE_ATTRS: [&str; 5] = [
    "query",
    "mutation",
    "subscription",
    "entity",
    "field_resolver",
];

/// What an `#[entity]` owes beyond what a `#[query]` owes, refused at its own
/// span rather than inside async-graphql's derive.
///
/// **Three, in the order they are checked**, and naming them is the point — a
/// count drifts the moment one is added:
///
/// 1. no `#[entity(...)]` arguments — the `@key` is read off the method's own;
/// 2. no `#[graphql(...)]` of the method's own;
/// 3. at least one argument, since those arguments *are* the key.
///
/// All three are async-graphql's rules reworded and re-spanned: it reports
/// "Entity need to have at least one key" against the `#[operations]`
/// attribute, followed by a cascade naming a generated type the developer never
/// wrote. A fourth — a `Result` return — was refused here while the guard chain
/// was compiled out of a bare-return operation; the chain now runs whatever the
/// method returns, so an entity's return type is as free as a query's.
/// async-graphql's "Must be asynchronous" is not among them: it
/// binds the method the expansion emits, which is always `async`, so a `fn` is
/// served like any operation's.
///
/// **Two more live outside this function**, because they are not the entity's
/// alone: `bind = Service` is refused in `resolver_impl_inner` (where the
/// posture is parsed), and `check_operations` refuses at boot an `#[entity]`
/// whose resolved type the registry keys nothing on — a fact only the registry
/// holds. Two others bind every operation, entity included: a misplaced
/// `&Context` and a `#[version]`.
fn entity_refusals(attr: &Attribute, other: &[Attribute], sig: &Signature) -> syn::Result<()> {
    // `#[entity(key = "id")]` is the first thing a developer arriving from
    // Apollo reaches for, and the key is not theirs to declare: async-graphql
    // reads it off the resolver's own arguments. Accepting and discarding it
    // would be the ignored argument the rules call silence.
    if !matches!(attr.meta, syn::Meta::Path(_)) {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[entity]` takes no arguments — the `@key` is inferred from this method's own \
             arguments, so an entity resolved by `id` is one taking `id`. Add or rename a \
             parameter to change the key",
        ));
    }
    // async-graphql's derive parses the **first** `graphql` attribute on a method
    // and removes exactly one, so a developer's `#[graphql(name = …)]` consumes
    // the slot and the `#[graphql(entity)]` this decorator emits is silently
    // dropped — the method stops being an entity resolver, and what the compiler
    // then reports is a leftover attribute against `#[operations]`. There is no
    // working spelling to redirect to, `#[graphql(entity, name = …)]` colliding
    // the same way, so the refusal names the limit rather than an alternative.
    if let Some(attr) = other.iter().find(|a| a.path().is_ident("graphql")) {
        return Err(syn::Error::new_spanned(
            attr,
            "an `#[entity]` takes no `#[graphql(...)]` of its own: async-graphql reads the \
             first one on a method and this decorator has to emit `#[graphql(entity)]` there, \
             so yours would silently take its place and the method would stop being an entity \
             resolver. Rename the method itself, or move what you were configuring to the \
             type's own `#[graphql(...)]`",
        ));
    }
    // No argument ⇒ no `@key` ⇒ async-graphql refuses the whole schema, from
    // inside its derive, naming a generated type.
    let ctx = ctx_param_ident(sig);
    let keys = sig.inputs.iter().filter(|arg| match arg {
        FnArg::Receiver(_) => false,
        FnArg::Typed(pt) => match &*pt.pat {
            syn::Pat::Ident(pi) => Some(&pi.ident) != ctx.as_ref(),
            _ => true,
        },
    });
    if keys.count() == 0 {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            "an `#[entity]` method needs at least one argument — those arguments *are* the \
             `@key` the router matches a reference against, so an entity resolver with none \
             is a type no router can address",
        ));
    }
    Ok(())
}

/// `#[operations]` on the impl: split `#[query]`/`#[mutation]` methods into
/// generated `#[Object]` roots and register them.
fn resolver_impl(item: ItemImpl) -> TokenStream {
    match resolver_impl_inner(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The `#[operations]` expansion, returning `syn::Result<TokenStream2>`
/// so its gates are unit-testable without the `proc_macro` bridge —
/// `resolver_impl` is the thin `proc_macro::TokenStream` wrapper, the same
/// `entry`/`crud` split `#[crud]` uses. The mandatory-posture check below is
/// security-load-bearing: a `#[query]`/`#[mutation]` carrying neither
/// `#[authorize(...)]` nor `#[public]` must be a compile error, never an
/// ungated, unmasked operation.
fn resolver_impl_inner(mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let self_ty = item.self_ty.clone();

    let base = impl_self_ident(&self_ty, "#[operations]")?;

    // Module-gating uses `TypeId::of::<Self>()` so `Self` must be `'static`.
    // Reject generics here for a friendly span — otherwise the user sees a
    // deep-in-macro `T: 'static` failure on the inventory submission.
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "`#[operations] impl` must be on a concrete, `'static` self type — \
             generic and lifetime parameters are not supported (the resolver's \
             `TypeId` is its container key, which requires `'static`)",
        ));
    }

    // `#[use_guards(...)]` belongs on the struct (provider scope), uniform
    // with `#[controller]` and `#[gateway]`. Catch the legacy impl-block
    // placement here with a redirect message — the impl-form has no other
    // role for it (the struct-form parses and exposes it via
    // `__nestrs_resolver_guard_specs()`).
    GRAPHQL_PAIR.reject_host_layers(&item.attrs)?;
    reject_http_only_layers(&item.attrs, "GraphQL", "resolver")?;

    let query_obj = format_ident!("__{}Query", base);
    let mutation_obj = format_ident!("__{}Mutation", base);
    let subscription_obj = format_ident!("__{}Subscription", base);

    let mut query_methods: Vec<TokenStream2> = Vec::new();
    let mut mutation_methods: Vec<TokenStream2> = Vec::new();
    let mut subscription_methods: Vec<TokenStream2> = Vec::new();
    // One `(method, resolved GraphQL type name)` per `#[entity]`, submitted with
    // the `Query` root — see where they are pushed.
    let mut entity_claims: Vec<TokenStream2> = Vec::new();
    // async-graphql wants one `#[ComplexObject]` per parent type, so a
    // resolver's `#[field_resolver]` methods for the same parent merge into one impl.
    let mut field_groups: Vec<(Type, Vec<TokenStream2>)> = Vec::new();
    // Extra access-contract deps on top of the struct's `#[inject]` keys:
    // per-method guards + `#[field_resolver]` `&Service` injections.
    // Resolver-scope guards live in the struct's `__nestrs_injected()`
    // (parallel to `#[controller]` / `#[gateway]`).
    let mut all_guard_paths: Vec<(Vec<TokenStream2>, Path)> = Vec::new();
    let mut field_dep_types: Vec<(Vec<TokenStream2>, Type)> = Vec::new();
    // Each field of a root — and of a parent a field resolver extends — resolves
    // to one method. async-graphql does not refuse two: its registry keeps the
    // last field of a name while its dispatch `match` keeps the first, so the
    // schema documents one method's arguments and runs the other's body.
    let mut declared = DispatchKeys::new(
        "#[operations]",
        "a schema resolves each field of a type with one method, so the SDL would document \
         one and the dispatch run the other — give one a distinct name",
    );

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // One method, one role. `#[entity]` beside `#[mutation]` is the shape
        // that motivates saying more: an entity is resolved **by reference**,
        // from the `_entities` field the router calls on the `Query` root, and
        // no other root has one — so that clause is added only when `#[entity]`
        // sits beside another role. Explaining `#[query]` + `#[mutation]` with
        // the federation root answers a question the developer did not ask.
        let written: Vec<String> = method
            .attrs
            .iter()
            .filter(|a| ROLE_ATTRS.iter().any(|name| a.path().is_ident(name)))
            .map(|a| nest_rs_codegen::key_as_written(a.path()))
            .collect();
        let why = if written.iter().any(|role| role == "entity")
            && written.iter().any(|role| role != "entity")
        {
            " An entity resolver is a `Query`-root field — the router reaches it through \
              `_entities`, which the `Mutation` and `Subscription` roots do not have."
        } else {
            ""
        };
        let Some(idx) =
            nest_rs_codegen::one_role_per_method("role", &method.attrs, &ROLE_ATTRS, why)?
        else {
            continue;
        };
        shared_receiver(method, "#[operations]", &base, HostBorrow::Host)?;
        nest_rs_codegen::concrete_signature(method, "#[operations]")?;
        // The operation travels with the method: its delegating method already
        // carries every attribute left on it, and these condition what the
        // expansion emits beside the roots.
        let cfgs = cfg_attrs(&method.attrs);

        let verb_attr = method.attrs.remove(idx);
        // Not on a `#[field_resolver]`: its position 1 is the **parent**, so a
        // `&Context` correctly comes second there — see `field_method`, which
        // forwards it.
        if !verb_attr.path().is_ident("field_resolver") {
            reject_misplaced_ctx(&method.sig)?;
        }
        // `#[version]` narrows an HTTP *route* out of the versions its
        // controller mounts. A GraphQL operation has no address to narrow, and
        // left alone this is `cannot find attribute `version` in this scope` —
        // which names neither the edge nor the reason, and reads as a missing
        // import. Same sentence as the one `#[resolver(version = …)]` gets: the
        // fact is the edge's, not the site's.
        if let Some(version) = method.attrs.iter().find(|a| a.path().is_ident("version")) {
            return Err(Edge::Graphql.refuse_version(version));
        }
        let is_entity = verb_attr.path().is_ident("entity");
        if is_entity {
            entity_refusals(&verb_attr, &method.attrs, &method.sig)?;
        }

        reject_http_only_layers(&method.attrs, "GraphQL", "operation")?;
        let method_guards = take_use_guards(&mut method.attrs)?;
        let force_method_guards = take_force_guards(&mut method.attrs)?;
        // The operation's access posture: `#[authorize(Action, Entity)]`
        // (class gate + automatic response masking) or `#[public]`
        // (deliberately ungated). Exactly one is required on every
        // `#[query]`/`#[mutation]` — see the posture check below.
        let authorize_spec = take_authorize(&mut method.attrs)?;
        let is_public = take_flag_attr(&mut method.attrs, "public")?;
        all_guard_paths.extend(
            method_guards
                .iter()
                .chain(&force_method_guards)
                .map(|guard| (cfgs.clone(), guard.clone())),
        );
        // A `#[field_resolver]` runs its resolver's `#[use_guards]` and its own
        // method's, never the app-wide pool: the root field it resolves under
        // ran that once for the request (`GraphqlSite::Field`). The resolver's
        // own guards do run — the parent may come from another resolver's
        // root, which never ran them.
        let is_field = verb_attr.path().is_ident("field_resolver");

        // The delegating method keeps the signature and any remaining attrs
        // (`#[graphql(...)]` belongs there); the inherent method holds the body.
        // Unfolded, because the delegate is a macro: async-graphql reads a plain
        // `#[cfg]` and nothing else, so a `#[cfg]` inside a `#[cfg_attr]` left
        // its dispatch naming a method that was compiled out.
        let mut deleg_attrs = delegated_attrs(&method.attrs);
        // What the schema calls this field, and the one method it may resolve to.
        if !is_entity {
            let field = match graphql_name(&method.attrs)? {
                Some(name) => name,
                None => {
                    let field = lower_camel(&method.sig.ident.unraw().to_string());
                    name_the_field(&mut deleg_attrs, &field)?;
                    field
                }
            };
            // A root's fields, and a parent's, are separate namespaces.
            let (identity, key) = if is_field {
                let parent = field_parent_label(&method.sig);
                (
                    format!("{parent} {field}"),
                    format!("#[field_resolver] {parent}.{field}"),
                )
            } else {
                let root = nest_rs_codegen::key_as_written(verb_attr.path());
                (format!("{root} {field}"), format!("#[{root}] {field}"))
            };
            declared.declare(
                Collision::Marker,
                "field",
                &identity,
                &key,
                &method.sig.ident,
                &cfgs,
                &verb_attr,
            )?;
        }
        let mut sig = method.sig.clone();
        // Give each argument a plain binding name before anything keys off it:
        // `Valid(Json(input)): Valid<Json<Dto>>` becomes `input: Valid<Json<Dto>>`
        // here, so the pipe detection, the wrapper signature (whose parameter
        // names are the SDL argument names) and the forwarded call all see one
        // ident. The developer's method keeps its pattern — this is a clone.
        normalize_forwarded_args(sig.inputs.iter_mut())?;
        let sig = sig;
        let method_name = method.sig.ident.clone();

        if is_field {
            // A field resolver runs per-row inside an operation whose posture
            // (`#[authorize]`/`#[public]`) was already enforced on the root
            // query/mutation — a posture attribute here would be a silent
            // no-op lie, so reject it (same stance as `#[public]` on a WS
            // `#[subscribe_message]`).
            if authorize_spec.is_some() || is_public {
                return Err(syn::Error::new_spanned(
                    &method.sig.ident,
                    "a `#[field_resolver]` inherits the operation's access posture — \
                     `#[authorize(...)]`/`#[public]` belong on the root `#[query]`/`#[mutation]`; \
                     for an extra per-field gate bind `#[use_guards(...)]` here",
                ));
            }
            // Field resolvers gate per-row — their resolver's `#[use_guards]` and
            // their own, never the pool again (`ChainSite::Field`).
            let field_label = format!("{}.{}", quote!(#self_ty), method_name);
            let (parent_ty, deleg, deps) = field_method(
                &self_ty,
                &deleg_attrs,
                &sig,
                &method_guards,
                &force_method_guards,
                &field_label,
            )?;
            field_dep_types.extend(deps.into_iter().map(|dep| (cfgs.clone(), dep)));
            let key = field_parent_key(&parent_ty);
            match field_groups
                .iter_mut()
                .find(|(ty, _)| field_parent_key(ty) == key)
            {
                Some((_, methods)) => methods.push(deleg),
                None => field_groups.push((parent_ty, vec![deleg])),
            }
        } else {
            // Posture is mandatory and fail-secure: an operation the developer
            // forgot to think about does not compile, instead of shipping
            // ungated and unmasked. Any return type carries either posture: the
            // gate's denial and a masking failure surface through the wrapper's
            // `Result`, whatever the developer's method returns.
            match (&authorize_spec, is_public) {
                (Some(_), true) => {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        nest_rs_codegen::posture_contradiction(),
                    ));
                }
                (None, false) if is_entity => {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "an `#[entity]` declares its access posture, and it is the one role where \
                         forgetting is invisible: the router calls `_entities` with a *reference* \
                         — `{__typename, <key fields>}` — for an entity the client never named, so \
                         an ungated one is readable from outside every `#[authorize]` in the \
                         schema. Write `#[authorize(Action, Entity)]` (class gate + response mask) \
                         or `#[public]`",
                    ));
                }
                (None, false) => {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        nest_rs_codegen::posture_required(
                            "`#[query]`/`#[mutation]`/`#[subscription]`/`#[entity]`",
                            "no `#[authorize]` gate and no response mask — `#[use_guards]` \
                             guards still run",
                        ),
                    ));
                }
                _ => {}
            }
            let root_kind = if verb_attr.path().is_ident("query") || is_entity {
                // An entity resolver is a `Query`-root field carrying
                // `#[graphql(entity)]`: async-graphql moves it out of the query
                // fields and behind `_entities`, and infers the `@key` from its
                // own arguments. Everything above that — the chain, the gate,
                // the pipes, the mask — is a `#[query]`'s, because what the
                // router calls is an operation like any other.
                RootKind::Query
            } else if verb_attr.path().is_ident("mutation") {
                RootKind::Mutation
            } else {
                RootKind::Subscription
            };
            // async-graphql's subscription derive builds paths out of the return
            // type, and Rust refuses an `impl Trait` inside a path (E0562): a
            // stream nested in another path — `GqlResult<impl Stream<…>>`, the
            // `use async_graphql::Result as GqlResult` idiom — cannot reach it,
            // and left to the derive it is a wall of errors. Read off where the
            // `impl` sits, never off a name; a `Result` alias around a named
            // stream is refused by its type instead (`call_as_result`).
            if root_kind == RootKind::Subscription
                && let Returned::Value(ty) = returned(&sig)
                && nests_impl_trait(&ty)
            {
                return Err(syn::Error::new_spanned(
                    &method.sig.output,
                    "a `#[subscription]` returns `impl Stream<Item = T>`, a stream type, or one \
                     of those inside `Result<…>` / `FieldResult<…>`. This return nests an `impl \
                     Trait` inside another path, and async-graphql's derive builds paths from \
                     the return type, where Rust refuses one (E0562): if it is a `Result` under \
                     another name, spell it `Result<…>`",
                ));
            }
            // `bind = Service`: the operation declares its subject as an
            // `Authorized<Action, E>` parameter; the wrapper exposes a by-id
            // GraphQL argument in its place, binds it through `bind_required`
            // (which mints the proof for the attribute's action), and forwards
            // it — so the resolver body never parses an id or touches raw ORM.
            // The HTTP `Bind<A, S>` extractor, expressed for GraphQL through the
            // posture attribute that already carries the action + entity
            // (declared once, no duplicate).
            // Pair the spec with its `bind` service only when set — carries the
            // action alongside so the prelude never re-derives it from the spec.
            if is_entity
                && let Some(spec) = authorize_spec.as_ref()
                && let Some(bind) = spec.bind.as_ref()
            {
                return Err(syn::Error::new_spanned(
                    bind,
                    "`bind = Service` cannot arm an `#[entity]`: it answers `NOT_FOUND` for a row \
                     that is absent and `FORBIDDEN` for one the ability withholds, which on a \
                     field the router addresses **by key** is an existence oracle — a caller \
                     learns which keys exist by asking for them. That distinction is right on a \
                     mutation, whose subject the caller already named. Load the row in the body \
                     instead (`CrudService::access`) and answer `None` for both, so a reference \
                     the caller may not resolve is indistinguishable from one that resolves to \
                     nothing",
                ));
            }
            let bind_info = match authorize_spec
                .as_ref()
                .and_then(|s| s.bind.as_ref().map(|b| (s, b)))
            {
                Some((spec, service)) => {
                    let Some(subject_ident) = authorized_param_ident(&sig) else {
                        return Err(syn::Error::new_spanned(
                            &method_name,
                            "`#[authorize(Action, bind = Service)]` needs a parameter of type \
                             `Authorized<Action, E>` to receive the bound subject — the action in \
                             the type must match the one in the attribute (e.g. \
                             `#[authorize(Update, bind = FilesService)]` ⇒ `Authorized<Update, FileEntity>`)",
                        ));
                    };
                    let id_ident = spec.id_arg.clone().unwrap_or_else(|| format_ident!("id"));
                    Some((
                        service.clone(),
                        subject_ident,
                        id_ident,
                        spec.action.clone(),
                    ))
                }
                None => None,
            };
            // The wrapper signature: with `bind`, the `Authorized<A, E>`
            // parameter (not a GraphQL `InputType`) is replaced by the `id`
            // string argument the SDL exposes; without `bind`, it is the
            // method's own.
            // Per-argument pipes: `Piped<P, T>` / `Valid<T>` parameters. The
            // wrapper exposes `T` on the wire, runs the pipe, and forwards the
            // carrier — the resolver body only ever calls the service.
            let piped = piped_args(&sig);
            // A pipe can reject, and the rejection reaches the client through the
            // wrapper's `Result` — so a bare-return operation takes one as readily
            // as a fallible one.
            // The wrapper signature strips both bind and pipe wrappers from the
            // wire: the `Authorized<A, E>` subject becomes the `id` string
            // argument, and each `Piped<P, T>` / `Valid<T>` becomes its wire
            // value type `T`. Everything else is the method's own.
            let wrapper_sig = {
                let mut s = sig.clone();
                for input in s.inputs.iter_mut() {
                    let FnArg::Typed(pt) = input else { continue };
                    let Some(arg_ident) = (match &*pt.pat {
                        syn::Pat::Ident(pi) => Some(pi.ident.clone()),
                        _ => None,
                    }) else {
                        continue;
                    };
                    if let Some((_, subject_ident, id_ident, _)) = &bind_info
                        && arg_ident == *subject_ident
                    {
                        *input = parse_quote!(#id_ident: ::std::string::String);
                        continue;
                    }
                    if let Some(pa) = piped.iter().find(|pa| pa.ident == arg_ident) {
                        let ty = &pa.value_ty;
                        *input = parse_quote!(#arg_ident: #ty);
                    }
                }
                s
            };
            let arg_idents = forwarded_arg_idents(&sig)?;
            // Forward the original args, swapping the subject ident for the
            // locally-bound `__subject` proof when `bind` is set.
            let call_args: Vec<TokenStream2> = arg_idents
                .iter()
                .map(|ident| match &bind_info {
                    Some((_, subject_ident, _, _)) if ident == subject_ident => {
                        quote!(__subject)
                    }
                    _ => quote!(#ident),
                })
                .collect();
            // By path, never `self.0.method(..)`: the root holds the resolver in
            // an `Arc`, and method lookup tries the `Arc` before it derefs, so a
            // trait method of the operation's name implemented for `Arc<T>` ran
            // instead.
            let call = call_as_result(
                &sig,
                await_if_async(
                    &sig,
                    quote! { <#self_ty>::#method_name(&*self.0, #(#call_args),*) },
                ),
                root_kind,
            );
            let role_label = if is_entity {
                ENTITY_ROLE
            } else {
                root_kind.label()
            };
            let route_label = format!("{role_label} {method_name}");
            // Same label the guard chain logs under, reused as the structured
            // field on a dropped subscription item so one grep answers "which
            // operation refused this?" whichever layer refused it.
            let route_label_lit = LitStr::new(&route_label, proc_macro2::Span::call_site());
            // Always emit the chain: even when the method declares no
            // method-scope guards, the struct may have declared
            // resolver-scope guards (read at runtime through
            // `__nestrs_resolver_guard_specs()`), and the app its pool. The
            // wrapper returns `Result` whatever the method does, so a denial
            // always has somewhere to go.
            let (mut gsig, gctx) = ensure_ctx_param(&wrapper_sig);
            gsig.output = wrapper_output(&sig);
            let checks = layered_resolver_chain(
                &self_ty,
                &method_guards,
                &force_method_guards,
                &gctx,
                &route_label,
                if is_entity {
                    ChainSite::Entity
                } else {
                    ChainSite::Operation
                },
            );
            // `#[authorize(A, E)]`: class gate before the call, automatic
            // response masking after it — the same two effects the HTTP
            // `Authorize<A, E>` extractor + response shaper carry, emitted
            // here so a hand-written operation writes neither by hand.
            // The entity the gate + mask act on: written explicitly, or — when
            // `bind = Service` is set and the entity was omitted — derived from
            // `<Service as CrudService>::Entity` so it is never retyped. Computed
            // from the spec at each use site (gate + mask), never an unwrap of a
            // separately-built `Option`.
            let authz_entity = |spec: &AuthorizeSpec| match &spec.entity {
                Some(entity) => quote!(#entity),
                None => {
                    let service = spec
                        .bind
                        .as_ref()
                        .expect("entity-less authorize requires bind");
                    quote!(<#service as ::nest_rs_seaorm::CrudService>::Entity)
                }
            };
            let gate = authorize_spec.as_ref().map(|spec| {
                let action = &spec.action;
                let entity = authz_entity(spec);
                quote! {
                    ::nest_rs_authz::graphql::authorize::<#action, #entity>(#gctx)?;
                }
            });
            // `bind = Service`: load + authorize the subject row from the id
            // argument and bind it to `__subject` before the call. Runs after
            // the class gate (cheap, no DB) so a class-denied caller never hits
            // the database. Missing row → NOT_FOUND, denied row → FORBIDDEN.
            let bind_prelude = bind_info.as_ref().map(|(service, _, id_ident, action)| {
                quote! {
                    let __subject = ::nest_rs_seaorm::graphql::bind_required::<#action, #service>(
                        #gctx, &#id_ident,
                    ).await?;
                }
            });
            let body = match authorize_spec.as_ref().filter(|spec| !spec.unmasked) {
                Some(spec) => {
                    let action = &spec.action;
                    let entity = authz_entity(spec);
                    match root_kind {
                        // A query or a mutation answers once, so the posture's
                        // mask runs once, over the value.
                        // Through the probe's mapper, so the mask reads the row
                        // inside a `Result` under another name rather than the
                        // `Result` around it.
                        RootKind::Query | RootKind::Mutation => quote! {
                            match #call {
                                ::core::result::Result::Ok(__out) => {
                                    use ::nest_rs_core::AnswerFallback as _;
                                    ::nest_rs_core::Answer(&__out).mapper()(
                                        __out,
                                        |__value| {
                                            ::nest_rs_authz::graphql::masked_value_for::<
                                                #action, #entity, _,
                                            >(#gctx, __value)
                                        },
                                    )
                                }
                                ::core::result::Result::Err(__err) =>
                                    ::core::result::Result::Err(__err),
                            }
                        },
                        // A subscription answers *many* times, and the gate ran
                        // once — at subscribe. So the mask moves onto the
                        // stream: every item is evaluated against **this**
                        // subscriber's ability before it is pushed, and one the
                        // ability refuses is dropped rather than nulled. Same
                        // policy as `mask_many` applies to a row in a list,
                        // which is what an item over time is.
                        RootKind::Subscription => quote! {
                            match #call {
                                ::core::result::Result::Ok(__stream) => ::core::result::Result::Ok(
                                    ::nest_rs_graphql::async_graphql::futures_util::StreamExt::filter_map(
                                        __stream,
                                        move |__item| ::core::future::ready(
                                            ::nest_rs_graphql::keep_masked_item(
                                                #route_label_lit,
                                                ::nest_rs_authz::graphql::masked_item_for::<
                                                    #action, #entity, _,
                                                >(#gctx, __item),
                                            ),
                                        ),
                                    ),
                                ),
                                ::core::result::Result::Err(__err) =>
                                    ::core::result::Result::Err(__err),
                            }
                        },
                    }
                }
                None => call,
            };
            // Run each per-argument pipe over its extracted wire value, rebinding
            // the parameter to the `Piped`/`Valid` carrier the body receives. A
            // rejected pipe surfaces as an `async_graphql::Error` carrying the
            // `PipeError` message. Runs after the class gate (a class-denied
            // caller never runs a pipe), before the call.
            let pipe_prelude = piped.iter().map(|pa| {
                let ident = &pa.ident;
                let ty = &pa.value_ty;
                let apply = match &pa.pipe {
                    Some(pipe) => quote!(::nest_rs_pipes::Piped::<#pipe, #ty>::apply(#ident)),
                    None => quote!(::nest_rs_pipes::Valid::<#ty>::apply(#ident)),
                };
                quote! {
                    let #ident = #apply.map_err(|__e| ::nest_rs_graphql::pipe_error(&__e))?;
                }
            });
            // The one token that makes it an entity resolver, and it is emitted
            // rather than written: `#[graphql(entity)]` is what calls
            // `add_keys`, which is what brings `_service` and `_entities` into
            // existence at all.
            // Spelled bare on purpose: `graphql` is an *inert helper* the
            // `#[Object]` derive reads off the method and strips, not a macro
            // path to resolve — qualifying it asks the compiler to find a
            // `graphql` item in async-graphql's root, which is not what it is.
            let entity_attr = is_entity.then(|| quote!(#[graphql(entity)]));
            // What this `#[entity]` claims to key, as a *runtime* pair: the
            // method's name, and the GraphQL type name async-graphql will resolve
            // it to. The boot check asks the registry whether that name came back
            // carrying a `@key` — `add_keys` returns silently for anything that
            // is not an object or an interface, which is how an `#[entity]`
            // returning `Vec<T>` compiled, booted, and registered nothing.
            if is_entity && matches!(sig.output, syn::ReturnType::Type(..)) {
                let claimed = LitStr::new(&method_name.to_string(), method_name.span());
                // The value the wrapper registers — `T` of the
                // `async_graphql::Result<T>` it returns, which is the type
                // `add_keys` reads — so the claim and the registry name one
                // type by construction.
                let value = returned(&sig).value();
                entity_claims.push(quote! {
                    #(#cfgs)*
                    (
                        #claimed,
                        <#value as ::nest_rs_graphql::async_graphql::OutputType>::type_name()
                            .into_owned(),
                    )
                });
            }
            // One operation line per dispatched field, and the span it is filed
            // under — the unit of work this edge opens. Emitted here, where
            // `#[operations]` composes every role's chain, so a query, a
            // mutation, an entity and a field resolver all get it rather than
            // whichever one asked.
            //
            // A subscription is the one role left out, and deliberately: its
            // unit is the connection, filed by `graphql.subscription` when the
            // socket ends. Wrapping it here would file a second line naming the
            // *subscribe*, which is not the work.
            let delegating = if root_kind == RootKind::Subscription {
                // async-graphql's `#[Subscription]` awaits the method it is given
                // before it has a stream to poll — the method *emitted here*, so
                // that one is `async` and the developer's is called with or
                // without an `.await`, as it is written.
                let mut gsig = gsig;
                gsig.asyncness = Some(syn::token::Async(proc_macro2::Span::call_site()));
                quote! {
                    #(#deleg_attrs)*
                    #entity_attr
                    #gsig { #checks #gate #bind_prelude #(#pipe_prelude)* #body }
                }
            } else {
                let role_lit = LitStr::new(role_label, proc_macro2::Span::call_site());
                // The wrapper always answers a `Result`, so a denial is an
                // `error` line whatever the developer's method returns.
                let succeeded = quote!(::core::result::Result::is_ok);
                // The wrapper is `async` whatever the developer's method is: the
                // unit is awaited, and the body it wraps already had to be —
                // every emitted guard chain awaits. The developer's own method
                // keeps its signature; `#call` decides whether to await it.
                let mut gsig = gsig;
                gsig.asyncness = Some(syn::token::Async(proc_macro2::Span::call_site()));
                quote! {
                    #(#deleg_attrs)*
                    #entity_attr
                    #gsig {
                        let __operation = ::nest_rs_graphql::GraphqlOperationContext::field(#gctx);
                        ::nest_rs_graphql::run_operation(
                            #role_lit,
                            __operation.name(),
                            #succeeded,
                            async move { #checks #gate #bind_prelude #(#pipe_prelude)* #body },
                        )
                        .await
                    }
                }
            };
            match root_kind {
                RootKind::Query => query_methods.push(delegating),
                RootKind::Mutation => mutation_methods.push(delegating),
                RootKind::Subscription => subscription_methods.push(delegating),
            }
        }

        // The developer's method keeps its prose, its `#[allow]`s and its
        // conditions: a method compiled out has to be compiled out here too,
        // where its body is, and a lint allowed on it — `non_snake_case` on the
        // `userID` it is named for — still names that method.
        method
            .attrs
            .retain(|a| a.path().is_ident("doc") || a.path().is_ident("allow"));
        for condition in &cfgs {
            method.attrs.extend(syn::parse::Parser::parse2(
                Attribute::parse_outer,
                condition.clone(),
            )?);
        }
        for input in method.sig.inputs.iter_mut() {
            if let FnArg::Typed(pt) = input {
                pt.attrs.clear();
            }
        }
    }

    let query_block = root_object(
        &query_obj,
        &self_ty,
        &query_methods,
        RootKind::Query,
        &entity_claims,
    );
    let mutation_block = root_object(
        &mutation_obj,
        &self_ty,
        &mutation_methods,
        RootKind::Mutation,
        &[],
    );
    let subscription_block = root_object(
        &subscription_obj,
        &self_ty,
        &subscription_methods,
        RootKind::Subscription,
        &[],
    );
    let field_blocks = field_groups.iter().map(|(parent_ty, methods)| {
        let root = async_graphql_root();
        let root_str = async_graphql_root_str();
        quote! {
            #[#root::ComplexObject(crate = #root_str)]
            impl #parent_ty {
                #(#methods)*
            }
        }
    });

    // `Discoverable::injected` = struct `#[inject]` keys + operation guards +
    // `#[field_resolver]` deps. `register` is a no-op: the schema builds the resolver
    // from the assembled container at boot.
    // Operation guards then `#[field_resolver]` deps, each with the label that
    // names it in a boot error. Two walks because the two lists have different
    // token types, concatenated in the order the keys were: `LayerDeps` keeps
    // each half internally aligned, and appending one to the other preserves it.
    let mut layers = layer_deps(
        all_guard_paths
            .iter()
            .map(|(cfgs, item)| Conditional { cfgs, item }),
    );
    let field_layers = layer_deps(
        field_dep_types
            .iter()
            .map(|(cfgs, item)| Conditional { cfgs, item }),
    );
    layers.keys.extend(field_layers.keys);
    layers.labels.extend(field_layers.labels);
    let injected_methods = injected_methods_with_layers(&self_ty, &layers);
    // Every guard declared at this site runs `Guard::check_graphql`, whose default
    // is `Ok(())` — so one bound per guard, failing at the `#[use_guards]` line
    // rather than passing every operation in silence.
    let capability_bounds = guard_capability_bounds(
        all_guard_paths
            .iter()
            .map(|(cfgs, item)| Conditional { cfgs, item }),
        quote!(::nest_rs_guards::GraphqlGuard),
    );

    let markers = declared.markers(&self_ty, &item.generics);

    Ok(quote! {
        #item

        #capability_bounds

        #markers

        #query_block
        #mutation_block
        #subscription_block
        #(#field_blocks)*

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_methods

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                builder
            }
        }
    })
}

/// Build a field resolver's `#[ComplexObject]` method. The inherent method's
/// first value argument is the parent (`parent: &ParentType`); the generated
/// method takes the parent as `&self`, builds the resolver from the container,
/// and delegates. Owned args become GraphQL field arguments; `&`-reference
/// args are injected (a `&Service` from the container or a `&DataLoader<…>`
/// from the request context) and never leak into the schema.
fn field_method(
    self_ty: &Type,
    deleg_attrs: &[Attribute],
    sig: &Signature,
    guards: &[Path],
    force_guards: &[Path],
    field_label: &str,
) -> syn::Result<(Type, TokenStream2, Vec<Type>)> {
    // Same normalization as an operation's: a destructured argument gets the
    // plain name the `#[ComplexObject]` method declares it under (and exposes in
    // the SDL) and forwards it by, while the developer's method keeps its
    // pattern. Working on a clone is what keeps that true.
    let owned_sig = {
        let mut s = sig.clone();
        normalize_forwarded_args(s.inputs.iter_mut())?;
        s
    };
    let sig = &owned_sig;

    // The receiver was read by `shared_receiver` before this was called.
    let mut inputs = sig.inputs.iter();
    inputs.next();

    let parent = inputs.next().ok_or_else(|| {
        syn::Error::new_spanned(
            sig,
            "#[field_resolver] method needs a parent argument `parent: &ParentType` — the object being resolved",
        )
    })?;
    let FnArg::Typed(parent) = parent else {
        return Err(syn::Error::new_spanned(
            parent,
            "#[field_resolver] parent argument must be typed",
        ));
    };
    let Type::Reference(parent_ref) = &*parent.ty else {
        return Err(syn::Error::new_spanned(
            &parent.ty,
            "#[field_resolver] parent argument must be a reference `&ParentType`",
        ));
    };
    let parent_ty = (*parent_ref.elem).clone();

    let rest: Vec<&FnArg> = inputs.collect();
    let rest_idents = forwarded_idents(rest.iter().copied())?;

    let method_name = &sig.ident;

    // An owned post-parent arg is a GraphQL field argument; a `&`-reference
    // is an injected dep (a `&T` is never a GraphQL `InputType`). A
    // `&DataLoader<…>` comes from the request context; any other `&service`
    // is a container singleton.
    let mut gql_args: Vec<&FnArg> = Vec::new();
    let mut call_args: Vec<TokenStream2> = Vec::new();
    let mut dep_bindings: Vec<TokenStream2> = Vec::new();
    // Container-resolved `&Service` types (dataloaders excluded), reported up
    // so the impl macro folds them into `Discoverable::injected`.
    let mut injected_deps: Vec<Type> = Vec::new();
    for (arg, ident) in rest.iter().copied().zip(&rest_idents) {
        let FnArg::Typed(pt) = arg else { continue };
        // The documented `#[field_resolver]` shape — `(&self, parent, ctx)` —
        // and the one site where a `&Context` legitimately follows another
        // parameter, because position 1 is the parent. Forwarded as the `__ctx`
        // the wrapper already holds. It used to fall through to the injected-dep
        // arm below, ask the container for a `Context`, find nothing, and answer
        // *no provider registered for `& Context < '_ >`* on **every request** —
        // a shape the docs teach, failing only once served.
        if ctx_ident_of(arg).is_some() {
            call_args.push(quote! { __ctx });
            continue;
        }
        if let Type::Reference(reference) = &*pt.ty {
            let dep_ty = &*reference.elem;
            let dep = format_ident!("__dep_{}", ident);
            if is_dataloader(dep_ty) {
                // `data_unchecked` panics only if `GraphqlModule` (and thus
                // the loader extension) was never imported.
                dep_bindings.push(quote! {
                    let #dep = __ctx.data_unchecked::<#dep_ty>();
                });
                call_args.push(quote! { #dep });
            } else {
                let msg = format!(
                    "#[field_resolver] `{}`: no provider registered for `{}`",
                    method_name,
                    quote!(#dep_ty),
                );
                // A missing provider degrades to a named GraphQL error, matching
                // the `data_opt` pattern the relation resolvers use — through the
                // wrapper's `Result`, so a bare-return resolver no longer panics
                // on a request path. The access graph has already validated the
                // dep at boot either way.
                dep_bindings.push(quote! {
                    let #dep = __container.get::<#dep_ty>().ok_or_else(|| {
                        ::nest_rs_graphql::async_graphql::Error::new(#msg)
                    })?;
                });
                call_args.push(quote! { &#dep });
                injected_deps.push(dep_ty.clone());
            }
        } else {
            call_args.push(quote! { #ident });
            gql_args.push(arg);
        }
    }

    let generics = &sig.generics;
    let where_clause = &sig.generics.where_clause;
    let output = wrapper_output(sig);

    // Always emitted, whatever the method returns: the resolver's own
    // `#[use_guards]` and the method's run here, and the app-wide pool does not
    // — the root field this resolves under ran it once for the request
    // (`GraphqlSite::Field`).
    let checks = layered_resolver_chain(
        self_ty,
        guards,
        force_guards,
        &format_ident!("__ctx"),
        field_label,
        ChainSite::Field,
    );
    // A field resolver is a dispatched unit of work like any other role — it
    // runs its own guard chain, hits its own services and has its own duration —
    // so it files the same line. `#[operations]` is where every role's chain is
    // composed, and this is that seam for the `#[ComplexObject]` half.
    let role_lit = LitStr::new(FIELD_ROLE, proc_macro2::Span::call_site());
    let succeeded = quote!(::core::result::Result::is_ok);
    // Always `async`, whatever the developer's method is — see the root
    // operation's wrapper. `#await_tok` still follows the inner method's own
    // spelling.
    // By path: the resolver is built by value here, and method lookup on a value
    // tries a trait method taking `self` before the `&self` it derefs to.
    // A field resolver answers once, as a query does.
    let call = call_as_result(
        sig,
        await_if_async(
            sig,
            quote! {
                <#self_ty>::#method_name(&__resolver, self #(, #call_args)*)
            },
        ),
        RootKind::Query,
    );
    let method = quote! {
        #(#deleg_attrs)*
        async fn #method_name #generics (
            &self,
            __ctx: &::nest_rs_graphql::async_graphql::Context<'_>
            #(, #gql_args)*
        ) #output #where_clause {
            let __operation = ::nest_rs_graphql::GraphqlOperationContext::field(__ctx);
            ::nest_rs_graphql::run_operation(
                #role_lit,
                __operation.name(),
                #succeeded,
                async move {
                    #checks
                    let __container = __ctx.data_unchecked::<::nest_rs_core::Container>();
                    #(#dep_bindings)*
                    let __resolver = <#self_ty>::from_container(__container);
                    #call
                },
            )
            .await
        }
    };
    Ok((parent_ty, method, injected_deps))
}

/// A `#[field_resolver]`'s parent type as written, for the field's identity —
/// the type of its first argument after the receiver.
fn field_parent_label(sig: &Signature) -> String {
    match sig.inputs.iter().nth(1) {
        Some(FnArg::Typed(typed)) => field_parent_key(match &*typed.ty {
            Type::Reference(reference) => &reference.elem,
            other => other,
        }),
        _ => String::new(),
    }
}

/// The `name = "…"` stated in a method's `#[graphql(...)]`, when one is.
fn graphql_name(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("graphql")) {
        let Ok(list) = attr.meta.require_list() else {
            continue;
        };
        let metas = list.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;
        for meta in metas {
            if let syn::Meta::NameValue(value) = meta
                && value.path.is_ident("name")
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(name),
                    ..
                }) = &value.value
            {
                return Ok(Some(name.value()));
            }
        }
    }
    Ok(None)
}

/// A method name as the schema serves it — **async-graphql's own rule**, so
/// stating it changes nothing a client sees: `get_2fa` is `get2Fa`, `a_1b` is
/// `a1B`, `page_2_items` is `page2Items`, `userID` stays `userID`.
///
/// async-graphql's default for a field is `Inflector`'s `to_camel_case`, which
/// starts a new word at a separator **and after a digit**, and keeps a capital
/// after a lowercase letter. The expansion states the name on every field it
/// emits ([`name_the_field`]) so the identity the duplicate check reads is the
/// name served; for that statement to be invisible it has to be the name
/// async-graphql would have served anyway. 7.0's first cut split at `_` only, so
/// `get_2fa` moved from `get2Fa` to `get2fa` and every client query naming it
/// broke with "unknown field" while the CHANGELOG said the SDL was unchanged.
///
/// Ported rather than depended on: `Inflector`'s last release is from 2019, past
/// the freshness bar for a new direct dependency, and the rule is these lines.
/// The regression test holds it against async-graphql's derive itself, so the
/// port cannot drift from the oracle unnoticed. Two methods this reads as one
/// field — `a1_b` and `a_1b` are both `a1B` — are refused by the duplicate
/// check, which is right: async-graphql would have served one of them.
fn lower_camel(name: &str) -> String {
    let trimmed = name.trim_end_matches(|c: char| !c.is_alphanumeric());
    let mut camel = String::with_capacity(trimmed.len());
    let mut new_word = false;
    let mut last = ' ';
    let mut started = false;
    for c in trimmed.chars() {
        if !c.is_alphanumeric() {
            // A separator starts a word once something has been written; a
            // leading one is skipped.
            new_word |= started;
        } else if c.is_numeric() {
            started = true;
            new_word = true;
            camel.push(c);
        } else if new_word || (last.is_lowercase() && c.is_uppercase()) {
            started = true;
            new_word = false;
            camel.push(c.to_ascii_uppercase());
        } else {
            started = true;
            last = c;
            camel.push(c.to_ascii_lowercase());
        }
    }
    camel
}

/// State `field` as the name async-graphql serves a method under, unless the
/// developer stated one.
///
/// async-graphql reads the **first** `#[graphql(...)]` on a method and strips
/// only that one, so a second attribute would be left behind as an unknown one.
/// The name therefore joins the developer's own `#[graphql(...)]` when there is
/// one, and is an attribute of its own when there is none.
fn name_the_field(attrs: &mut Vec<Attribute>, field: &str) -> syn::Result<()> {
    let name = LitStr::new(field, proc_macro2::Span::call_site());
    match attrs
        .iter_mut()
        .find(|attr| attr.path().is_ident("graphql"))
    {
        Some(attr) => {
            let list = attr.meta.require_list()?;
            let path = &list.path;
            let tokens = &list.tokens;
            let comma = (!tokens.is_empty()).then(|| quote!(,));
            *attr = parse_quote!(#[#path(#tokens #comma name = #name)]);
        }
        None => attrs.push(parse_quote!(#[graphql(name = #name)])),
    }
    Ok(())
}

/// `DataLoader<…>` matched on the final path segment, so both bare and
/// fully-qualified forms are recognised.
fn is_dataloader(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp
        .path
        .segments
        .last()
        .is_some_and(|s| s.ident == "DataLoader"))
}

/// The framework's re-export of async-graphql — the root every emitted
/// async-graphql attribute is pinned to.
fn async_graphql_root() -> TokenStream2 {
    quote!(::nest_rs_graphql::async_graphql)
}

/// The same root as the **string** a `crate = ` argument takes, built from
/// [`async_graphql_root`]'s tokens rather than re-typed: a path that no longer
/// resolves is a compile error here, while a stale string parses fine and
/// silently sends the expansion back to the call site's prelude — the failure
/// the override exists to close.
fn async_graphql_root_str() -> String {
    async_graphql_root()
        .into_iter()
        .map(|t| t.to_string())
        .collect()
}

/// Which async-graphql root a set of operations becomes.
///
/// Everything that differs between the three roots is answered here — the
/// registry variant, the derive attribute, the registry entry point, the built
/// member and the log label. A fourth root would be a variant plus five arms,
/// and the compiler names every one it forgot; the alternative (a bare
/// `TokenStream2` kind threaded through `root_object`) named none of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RootKind {
    Query,
    Mutation,
    Subscription,
}

impl RootKind {
    /// The `GraphqlResolverKind` variant this root registers under.
    fn variant(self) -> TokenStream2 {
        match self {
            Self::Query => quote!(Query),
            Self::Mutation => quote!(Mutation),
            Self::Subscription => quote!(Subscription),
        }
    }

    /// The async-graphql derive that builds the generated root type.
    fn derive(self) -> Ident {
        match self {
            Self::Query | Self::Mutation => format_ident!("Object"),
            Self::Subscription => format_ident!("Subscription"),
        }
    }

    /// The registry call that yields the root's `MetaType`. A `#[Subscription]`
    /// type is not an `OutputType`, so it takes its own entry point.
    fn fake_type(self) -> Ident {
        match self {
            Self::Query | Self::Mutation => format_ident!("create_fake_output_type"),
            Self::Subscription => format_ident!("create_fake_subscription_type"),
        }
    }

    /// The `GraphqlRootMember` variant the built root is handed back as.
    fn member(self) -> TokenStream2 {
        match self {
            Self::Query | Self::Mutation => quote!(Object),
            Self::Subscription => quote!(Subscription),
        }
    }

    /// The spec's word for the operation, used in guard-chain and denial logs.
    fn label(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
            Self::Subscription => "subscription",
        }
    }
}

/// The role word an `#[entity]` files under — async-graphql's `_entities`
/// resolves it, so it is neither of the three [`RootKind`] words.
const ENTITY_ROLE: &str = "entity";

/// The role word a `#[field_resolver]` files under. Beside [`RootKind::label`]
/// rather than inline at its emitter, so the four words a line's `role` can take
/// are one list.
const FIELD_ROLE: &str = "field";

fn root_object(
    obj: &Ident,
    self_ty: &Type,
    methods: &[TokenStream2],
    kind: RootKind,
    entity_claims: &[TokenStream2],
) -> TokenStream2 {
    if methods.is_empty() {
        return quote!();
    }
    // Resolver struct name, logged beside each mounted operation at boot.
    let resolver_name = impl_self_ident(self_ty, "#[operations]")
        .map(|i| i.to_string())
        .unwrap_or_else(|_| "resolver".to_string());
    let resolver_name = LitStr::new(&resolver_name, proc_macro2::Span::call_site());
    let root = async_graphql_root();
    let root_str = async_graphql_root_str();
    let derive = kind.derive();
    let variant = kind.variant();
    let fake_type = kind.fake_type();
    let member = kind.member();
    quote! {
        #[allow(non_camel_case_types)]
        pub struct #obj(::std::sync::Arc<#self_ty>);

        // `crate = ` pins async-graphql's own expansion to the umbrella's
        // re-export. Without it the derive asks `proc-macro-crate` what the
        // *call site* declared and falls back to a bare `::async_graphql`, so
        // every app that installed `nest-rs` with the `graphql` feature — the
        // documented line, and the only one — failed to compile inside this
        // attribute. Witnessed by `nest-rs-macro-hygiene`'s `resolver` module.
        #[#root::#derive(crate = #root_str)]
        impl #obj {
            #(#methods)*
        }

        ::nest_rs_graphql::inventory::submit! {
            ::nest_rs_graphql::GraphqlResolverRegistration {
                kind: ::nest_rs_graphql::GraphqlResolverKind::#variant,
                resolver_name: #resolver_name,
                resolver_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                entities: || ::std::vec![#(#entity_claims),*],
                type_info: |__r| __r.#fake_type::<#obj>(),
                build: |__c| ::nest_rs_graphql::GraphqlRootMember::#member(
                    ::std::boxed::Box::new(
                        #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                    ),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    // Every `#[query]`/`#[mutation]` must declare an access posture. This gate
    // is security-load-bearing: an operation the developer forgot to think
    // about must *not compile* rather than ship ungated and unmasked. A posture
    // regression here would silently expose data, so the compile error is the
    // guarantee — pinned by asserting the expansion fails and the diagnostic
    // names the rule.
    #[test]
    fn query_without_posture_fails_to_expand() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[query]
                async fn things(&self) -> ::std::vec::Vec<Thing> {
                    ::std::vec::Vec::new()
                }
            }
        };
        let err = resolver_impl_inner(item)
            .expect_err("a query with neither #[authorize] nor #[public] must fail to expand");
        let msg = err.to_string();
        assert!(
            msg.contains("posture"),
            "diagnostic names the posture rule: {msg}"
        );
        assert!(
            msg.contains("#[authorize"),
            "diagnostic points at #[authorize]: {msg}"
        );
        assert!(
            msg.contains("#[public]"),
            "diagnostic points at #[public]: {msg}"
        );
    }

    // The same gate for a `#[mutation]` — a write operation with no posture is
    // exactly the case that must never slip through.
    #[test]
    fn mutation_without_posture_fails_to_expand() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[mutation]
                async fn make_thing(&self) -> ::nest_rs_graphql::async_graphql::Result<Thing> {
                    ::core::result::Result::Ok(Thing)
                }
            }
        };
        let err = resolver_impl_inner(item)
            .expect_err("a mutation with no declared posture must fail to expand");
        assert!(err.to_string().contains("posture"), "{}", err);
    }

    // `#[public]` is a valid posture: the operation is deliberately ungated, so
    // it expands.
    #[test]
    fn public_query_expands() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[query]
                #[public]
                async fn ping(&self) -> i32 {
                    0
                }
            }
        };
        resolver_impl_inner(item).expect("a #[public] query expands");
    }

    // `#[authorize(Action, Entity)]` is the other valid posture (class gate +
    // automatic response mask); it expands.
    #[test]
    fn authorized_query_expands() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[query]
                #[authorize(::nest_rs_authz::Read, Thing)]
                async fn thing(&self) -> ::nest_rs_graphql::async_graphql::Result<Thing> {
                    ::core::result::Result::Ok(Thing)
                }
            }
        };
        resolver_impl_inner(item).expect("an #[authorize(...)] query expands");
    }

    // Declaring both postures is a contradiction — an operation is gated or
    // public, never both.
    #[test]
    fn authorize_and_public_together_fail_to_expand() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[query]
                #[authorize(::nest_rs_authz::Read, Thing)]
                #[public]
                async fn thing(&self) -> ::nest_rs_graphql::async_graphql::Result<Thing> {
                    ::core::result::Result::Ok(Thing)
                }
            }
        };
        let err = resolver_impl_inner(item)
            .expect_err("#[authorize] and #[public] together must fail to expand");
        assert!(err.to_string().contains("contradict"), "{}", err);
    }

    /// The served name is async-graphql's (`Inflector`'s `to_camel_case`): a
    /// word starts after `_` and after a digit, a capital after a lowercase
    /// letter is kept, and the rest is lowercased.
    #[test]
    fn a_method_name_is_camel_cased_as_async_graphql_does() {
        for (method, served) in [
            ("user_count", "userCount"),
            ("get_2fa", "get2Fa"),
            ("a_1b", "a1B"),
            ("a1_b", "a1B"),
            ("page_2_items", "page2Items"),
            ("v2_api", "v2Api"),
            ("userID", "userID"),
            ("user_id", "userId"),
            ("_leading", "leading"),
            ("trailing_", "trailing"),
            ("x", "x"),
        ] {
            assert_eq!(lower_camel(method), served, "{method}");
        }
    }

    /// Every role's wrapper runs the guard chain and answers a `Result`,
    /// whatever the developer's method returns. The chain used to be compiled
    /// out of a bare-return operation, so a resolver-scope or app-wide guard
    /// protected only the operations that happened to return `Result`.
    #[test]
    fn a_bare_return_operation_runs_the_chain_through_a_result_wrapper() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[query]
                #[public]
                async fn count(&self) -> i32 { 0 }

                #[query]
                #[authorize(::nest_rs_authz::Read, Thing)]
                async fn thing(&self) -> Thing { Thing }

                #[subscription]
                #[public]
                async fn ticks(&self) -> impl Stream<Item = i32> { stream() }

                #[field_resolver]
                async fn label(&self, parent: &Thing) -> String { String::new() }
            }
        };
        let expanded = resolver_impl_inner(item)
            .expect("a bare-return operation expands under either posture")
            .to_string();
        assert_eq!(
            expanded.matches("run_layered_graphql_chain").count(),
            4,
            "one chain per role: {expanded}",
        );
        for wrapped in [
            "async_graphql :: Result < i32 >",
            "async_graphql :: Result < Thing >",
            "async_graphql :: Result < impl Stream < Item = i32 > >",
            "async_graphql :: Result < String >",
        ] {
            assert!(
                expanded.contains(wrapped),
                "missing `{wrapped}`: {expanded}"
            );
        }
        assert!(
            expanded.contains("GraphqlSite :: Field"),
            "a field resolver names its own site: {expanded}",
        );
        assert!(
            !expanded.contains(". expect ("),
            "no request-path panic in a wrapper: {expanded}",
        );
    }
}
