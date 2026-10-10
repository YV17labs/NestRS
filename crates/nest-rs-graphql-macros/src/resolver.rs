//! `#[resolver]`: construction on the struct. `#[operations]`: the
//! orchestration of the `#[query]` / `#[mutation]` / `#[subscription]` /
//! `#[field_resolver]` methods on its impl block.

use nest_rs_codegen::pair;
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
    Collision, Conditional, DispatchKeys, HostBorrow, InjectableBody, PipeWrapper, VersionedEdge,
    await_if_async, build_injectable_body, cfg_attrs, delegated_attrs, force_guard_typeids,
    forwarded_arg_idents, forwarded_idents, from_container_method, guard_capability_bounds,
    impl_self_ident, injected_keys_with_layers, injected_methods_with_layers,
    injected_names_with_layers, layer_deps, normalize_forwarded_args, pipe_wrapper,
    reject_http_only_layers, scoped_specs, shared_receiver, take_flag_attr, take_path_list,
};

pub(crate) fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    // Before the blanket refusal, so `version = "…"` gets its own sentence.
    if let Err(err) = VersionedEdge::Graphql.reject_version(&args) {
        return err.to_compile_error().into();
    }
    if let Err(err) = reject_resolver_args(&args) {
        return err.to_compile_error().into();
    }

    match pair::GRAPHQL.parse_host(input.into()) {
        Ok(item) => resolver_struct(item),
        Err(err) => err.to_compile_error().into(),
    }
}

pub(crate) fn operations(args: TokenStream, input: TokenStream) -> TokenStream {
    // Refused through the kept item: dropping the `impl` buries the real error
    // under `no method found` at every caller.
    let written = TokenStream2::from(input.clone());
    let expansion: TokenStream = match pair::GRAPHQL
        .reject_args(
            &TokenStream2::from(args),
            "a resolver's construction and provider-scope layers are declared by",
        )
        .and_then(|()| pair::GRAPHQL.parse_operations(input.into()))
    {
        Ok(item) => resolver_impl(item),
        Err(err) => err.to_compile_error().into(),
    };
    pair::GRAPHQL
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

/// What `#[operations]` consumes off a method beside the layers and the posture.
const OPERATIONS_HELPERS: [&str; 6] = [
    "query",
    "mutation",
    "subscription",
    "entity",
    "field_resolver",
    "graphql",
];

fn reject_resolver_args(args: &TokenStream2) -> syn::Result<()> {
    if args.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new_spanned(
        args,
        format!(
            "{} takes no arguments; tag methods with {} under {}",
            pair::GRAPHQL.host(),
            pair::GRAPHQL.collects(),
            pair::GRAPHQL.operations()
        ),
    ))
}

/// `#[resolver]` on the struct: construction + provider-scope layer
/// declarations, read back by `#[operations]` through `__nestrs_resolver_*_specs()`.
fn resolver_struct(mut item: ItemStruct) -> TokenStream {
    if let Err(err) = reject_http_only_layers(&item.attrs, "GraphQL", "resolver") {
        return err.to_compile_error().into();
    }
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
    // Keys and index-aligned labels from one walk, so a missing guard is named at boot.
    let layers = layer_deps(guards.iter());
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::GraphqlGuard));

    // A generic resolver has no single `TypeId`, so it cannot be a `providers` entry.
    let descriptor = if item.generics.params.is_empty() {
        quote! {
            ::nest_rs_graphql::__private::inventory::submit! {
                ::nest_rs_graphql::__private::ResolverDescriptor {
                    resolver: || ::core::any::TypeId::of::<#name>(),
                    name: #name_str,
                }
            }
        }
    } else {
        quote!()
    };

    let residency = pair::GRAPHQL.host_residency(&name, &item.generics);

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

fn take_use_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    take_path_list(attrs, "use_guards")
}

fn take_force_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    take_path_list(attrs, "force_guards")
}

/// `#[authorize(Action, Entity)]` parsed off an operation: its declared access posture.
struct AuthorizeSpec {
    action: Path,
    /// `None` when derived from `<Service as CrudService>::Entity` under `bind`.
    entity: Option<Path>,
    unmasked: bool,
    /// `bind = Service`: a by-id argument loaded into the `Authorized<Action, E>` parameter.
    bind: Option<Path>,
    /// The synthesized id argument's snake_case name (async-graphql camelCases
    /// it); `None` is `id`.
    id_arg: Option<Ident>,
}

/// One token in `#[authorize(...)]`; a keyed one keeps its key, where a repeat is spanned.
enum AuthorizeArg {
    Positional(Path),
    Bind(Ident, Path),
    IdArg(Ident, Ident),
}

const AUTHORIZE: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("authorize", &["bind", "id_arg"]);

impl syn::parse::Parse for AuthorizeArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
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
                    nest_rs_codegen::unknown_argument("authorize", &spelled, AUTHORIZE.keys()),
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

fn refused(key: &'static str, takes: &'static str) -> impl Fn(syn::Error) -> syn::Error {
    move |stopped| {
        syn::Error::new(
            stopped.span(),
            nest_rs_codegen::takes_value("authorize", Some(key), takes),
        )
    }
}

const AUTHORIZE_SHAPE: &str = "expected `#[authorize(Action, Entity)]` — e.g. \
     `#[authorize(Read, users::Entity)]`; append `unmasked` to keep the class gate but mask the \
     response yourself. `bind = Service` (optionally `id_arg = ident`) binds the subject from an \
     id argument, and lets the entity be omitted (derived from `Service::Entity`): \
     `#[authorize(Update, bind = ArtworksService)]`";

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
    // A repeated key is refused, never last-write-wins: `bind` picks the loading service.
    let mut keys: Vec<Ident> = Vec::new();
    for arg in args {
        match arg {
            AuthorizeArg::Positional(p) => positional.push(p),
            AuthorizeArg::Bind(key, p) => {
                keys.push(key);
                bind = Some(p);
            }
            AuthorizeArg::IdArg(key, i) => {
                keys.push(key);
                id_arg = Some(i);
            }
        }
    }
    AUTHORIZE.take_all(&keys)?;
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

/// The ident of the parameter typed `Authorized<A, E>`, matched on the last path segment.
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
/// segment**, so `User` and `crate::wire::User` share one `#[ComplexObject]`
/// rather than colliding as `E0119`.
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

/// An operation parameter typed `Piped<P, T>` or `Valid<T>`: the wrapper exposes
/// `T` in its place and hands the operation the carrier.
struct PipedArg {
    ident: Ident,
    /// `None` for `Valid<T>`.
    pipe: Option<Path>,
    value_ty: Type,
}

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
/// Fallible when its last segment is exactly `Result` or `FieldResult` with a
/// type argument, mirroring `async-graphql-derive`'s `OutputType::parse`. A
/// renamed `Result` reads as a value and is split by `nest_rs_core::__private::Answer`
/// ([`call_as_result`]).
enum Returned<'a> {
    Fallible(&'a Type),
    /// `()` for a method that declares no return type.
    Value(Box<Type>),
}

impl Returned<'_> {
    fn value(&self) -> Type {
        match self {
            Self::Fallible(ty) => (*ty).clone(),
            Self::Value(ty) => (**ty).clone(),
        }
    }
}

/// Whether `ty` holds an `impl Trait` anywhere but as the whole of itself.
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

pub(crate) fn ctx_param_ident(sig: &Signature) -> Option<Ident> {
    sig.inputs.iter().find_map(ctx_ident_of)
}

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
/// the receiver: async-graphql reads a later one as a schema argument.
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

/// Ensure the delegating signature has a `&Context`, inserted directly after
/// `&self`, the only place async-graphql recognises it.
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

/// Emit the layered guard chain for a resolver operation: global +
/// resolver-scope + per-method guards, deduped by `TypeId`.
///
/// **Always emitted, whatever the operation returns**: the failure channel is
/// the wrapper's ([`wrapper_output`]), never the developer's method
/// (`.claude/decisions/operation-chain-any-return.md`).
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
            static __NESTRS_GUARD_CHAIN: ::nest_rs_guards::__private::SiteChainCell =
                ::nest_rs_guards::__private::SiteChainCell::new();
            let __container = #ctx.data_unchecked::<::nest_rs_core::Container>();
            ::nest_rs_guards::run_layered_graphql_chain(
                #ctx,
                __container,
                &__NESTRS_GUARD_CHAIN,
                #label_lit,
                &|| ::nest_rs_guards::__private::SiteChainSources {
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
/// `async_graphql::Result<T>`, so the chain's `?` never bounds the developer's error.
fn wrapper_output(sig: &Signature) -> syn::ReturnType {
    let value = returned(sig).value();
    parse_quote!(-> ::nest_rs_graphql::async_graphql::Result<#value>)
}

/// The developer's call as the wrapper's `async_graphql::Result<T>`. A value
/// return goes through `nest_rs_core::__private::Answer`, which knows a `Result` by its type;
/// a `#[subscription]`'s is refused there at compile time instead.
fn call_as_result(sig: &Signature, call: TokenStream2, root: RootKind) -> TokenStream2 {
    if let Returned::Fallible(_) = returned(sig) {
        // By type, never `Into`: an error nobody meant for the client answers
        // opaquely, its chain logged (`nest_rs_graphql`'s error tiers). The
        // report is built by its constructor, never by a call here: on a
        // `Result<T, !>` that call would be unreachable code.
        return quote! {
            ::core::result::Result::map_err(
                ::core::result::Result::map_err(#call, ::nest_rs_graphql::__private::ErrorReport),
                |__nestrs_report| {
                    #[allow(unused_imports)]
                    use ::nest_rs_graphql::__private::{ErrorReportChain as _, ErrorReportDeliberate as _};
                    __nestrs_report.into_graphql_error()
                },
            )
        };
    }
    if root == RootKind::Subscription {
        let span = match &sig.output {
            syn::ReturnType::Type(_, ty) => ty.span(),
            syn::ReturnType::Default => sig.ident.span(),
        };
        let probe = quote_spanned! {span=>
            ::nest_rs_graphql::__private::answers_a_stream(::nest_rs_core::__private::Answer(&__answer).kind());
        };
        return quote! {{
            use ::nest_rs_core::__private::AnswerFallback as _;
            let __answer = #call;
            #probe
            ::core::result::Result::<_, ::nest_rs_graphql::async_graphql::Error>::Ok(__answer)
        }};
    }
    quote! {{
        use ::nest_rs_core::__private::AnswerFallback as _;
        let __answer = #call;
        ::nest_rs_core::__private::Answer(&__answer).split::<::nest_rs_graphql::async_graphql::Error>()(
            __answer,
        )
    }}
}

const ROLE_ATTRS: [&str; 5] = [
    "query",
    "mutation",
    "subscription",
    "entity",
    "field_resolver",
];

/// What an `#[entity]` owes beyond what a `#[query]` owes, refused at its own
/// span rather than inside async-graphql's derive:
///
/// 1. no `#[entity(...)]` arguments — the `@key` is read off the method's own;
/// 2. no `#[graphql(...)]` of the method's own;
/// 3. at least one argument, since those arguments *are* the key.
fn entity_refusals(attr: &Attribute, other: &[Attribute], sig: &Signature) -> syn::Result<()> {
    if !matches!(attr.meta, syn::Meta::Path(_)) {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[entity]` takes no arguments — the `@key` is inferred from this method's own \
             arguments, so an entity resolved by `id` is one taking `id`. Add or rename a \
             parameter to change the key",
        ));
    }
    // async-graphql's derive reads only the first `graphql` attribute on a method.
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

/// The `#[operations]` expansion, unit-testable without the `proc_macro` bridge.
/// The mandatory-posture check is security-load-bearing.
fn resolver_impl_inner(mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let self_ty = item.self_ty.clone();

    let base = impl_self_ident(&self_ty, "#[operations]")?;

    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "`#[operations] impl` must be on a concrete, `'static` self type — \
             generic and lifetime parameters are not supported (the resolver's \
             `TypeId` is its container key, which requires `'static`)",
        ));
    }

    pair::GRAPHQL.reject_host_layers(&item.attrs)?;
    reject_http_only_layers(&item.attrs, "GraphQL", "resolver")?;

    let query_obj = format_ident!("__{}Query", base);
    let mutation_obj = format_ident!("__{}Mutation", base);
    let subscription_obj = format_ident!("__{}Subscription", base);

    let mut query_methods: Vec<TokenStream2> = Vec::new();
    let mut mutation_methods: Vec<TokenStream2> = Vec::new();
    let mut subscription_methods: Vec<TokenStream2> = Vec::new();
    let mut entity_claims: Vec<TokenStream2> = Vec::new();
    // async-graphql allows one `#[ComplexObject]` per parent type.
    let mut field_groups: Vec<(Type, Vec<TokenStream2>)> = Vec::new();
    let mut all_guard_paths: Vec<(Vec<TokenStream2>, Path)> = Vec::new();
    let mut field_dep_types: Vec<(Vec<TokenStream2>, Type)> = Vec::new();
    // async-graphql does not refuse two methods on one field: its registry keeps
    // the last, its dispatch the first.
    let mut declared = DispatchKeys::new(
        "#[operations]",
        "a schema resolves each field of a type with one method, so the SDL would document \
         one and the dispatch run the other — give one a distinct name",
    );

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

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
        let cfgs = cfg_attrs(&method.attrs);

        let verb_attr = method.attrs.remove(idx);
        // A `#[field_resolver]`'s position 1 is the parent; its `&Context` comes second.
        if !verb_attr.path().is_ident("field_resolver") {
            reject_misplaced_ctx(&method.sig)?;
        }
        if let Some(version) = method.attrs.iter().find(|a| a.path().is_ident("version")) {
            return Err(VersionedEdge::Graphql.refuse_version(version));
        }
        let is_entity = verb_attr.path().is_ident("entity");
        if is_entity {
            entity_refusals(&verb_attr, &method.attrs, &method.sig)?;
        }

        reject_http_only_layers(&method.attrs, "GraphQL", "operation")?;
        let method_guards = take_use_guards(&mut method.attrs)?;
        let force_method_guards = take_force_guards(&mut method.attrs)?;
        let authorize_spec = take_authorize(&mut method.attrs)?;
        let is_public = take_flag_attr(&mut method.attrs, "public")?;
        all_guard_paths.extend(
            method_guards
                .iter()
                .chain(&force_method_guards)
                .map(|guard| (cfgs.clone(), guard.clone())),
        );
        let is_field = verb_attr.path().is_ident("field_resolver");

        // Unfolded: async-graphql reads a plain `#[cfg]`, never one inside `#[cfg_attr]`.
        let mut deleg_attrs = delegated_attrs(&method.attrs);
        if !is_entity {
            let field = match graphql_name(&method.attrs)? {
                Some(name) => name,
                None => {
                    let field = lower_camel(&method.sig.ident.unraw().to_string());
                    name_the_field(&mut deleg_attrs, &field)?;
                    field
                }
            };
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
        // A plain binding name per argument: the wrapper's parameter names are the SDL's.
        normalize_forwarded_args(sig.inputs.iter_mut())?;
        let sig = sig;
        let method_name = method.sig.ident.clone();

        if is_field {
            if authorize_spec.is_some() || is_public {
                return Err(syn::Error::new_spanned(
                    &method.sig.ident,
                    "a `#[field_resolver]` inherits the operation's access posture — \
                     `#[authorize(...)]`/`#[public]` belong on the root `#[query]`/`#[mutation]`; \
                     for an extra per-field gate bind `#[use_guards(...)]` here",
                ));
            }
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
            // Posture is mandatory and fail-secure.
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
                // An entity resolver is a `Query`-root field, moved behind `_entities`.
                RootKind::Query
            } else if verb_attr.path().is_ident("mutation") {
                RootKind::Mutation
            } else {
                RootKind::Subscription
            };
            // async-graphql's subscription derive builds paths out of the return
            // type, and Rust refuses an `impl Trait` inside a path (E0562).
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
            let piped = piped_args(&sig);
            // On the wire, the bound subject becomes its `id` string and each pipe its `T`.
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
            let call_args: Vec<TokenStream2> = arg_idents
                .iter()
                .map(|ident| match &bind_info {
                    Some((_, subject_ident, _, _)) if ident == subject_ident => {
                        quote!(__subject)
                    }
                    _ => quote!(#ident),
                })
                .collect();
            // By path, never `self.0.method(..)`: method lookup tries the `Arc` first, so
            // a same-named trait method on `Arc<T>` would run instead.
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
            let route_label_lit = LitStr::new(&route_label, proc_macro2::Span::call_site());
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
            let authz_entity = |spec: &AuthorizeSpec| match &spec.entity {
                Some(entity) => quote!(#entity),
                None => {
                    #[expect(
                        clippy::expect_used,
                        reason = "a compile-time invariant of the parse above; a panic in a proc macro is a compile error"
                    )]
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
            // After the class gate, so a class-denied caller never reaches the database.
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
                        // Through the probe's mapper, so the mask reads the row
                        // inside a renamed `Result`, not the `Result` itself.
                        RootKind::Query | RootKind::Mutation => quote! {
                            match #call {
                                ::core::result::Result::Ok(__out) => {
                                    use ::nest_rs_core::__private::AnswerFallback as _;
                                    ::nest_rs_core::__private::Answer(&__out).mapper()(
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
                        // The gate ran once, at subscribe: every item is masked, a
                        // refused one dropped rather than nulled.
                        RootKind::Subscription => quote! {
                            match #call {
                                ::core::result::Result::Ok(__stream) => ::core::result::Result::Ok(
                                    ::nest_rs_graphql::async_graphql::futures_util::StreamExt::filter_map(
                                        __stream,
                                        move |__item| ::core::future::ready(
                                            ::nest_rs_graphql::__private::keep_masked_item(
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
            // Bare on purpose: `graphql` is an inert helper the `#[Object]` derive
            // strips, not a path to resolve.
            let entity_attr = is_entity.then(|| quote!(#[graphql(entity)]));
            // Checked at boot: `add_keys` returns silently for anything not an object
            // or an interface (a `Vec<T>`).
            if is_entity && matches!(sig.output, syn::ReturnType::Type(..)) {
                let claimed = LitStr::new(&method_name.to_string(), method_name.span());
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
            // No operation line for a subscription: its unit is the connection.
            let delegating = if root_kind == RootKind::Subscription {
                // async-graphql's `#[Subscription]` awaits the method it is given.
                let mut gsig = gsig;
                gsig.asyncness = Some(syn::token::Async(proc_macro2::Span::call_site()));
                quote! {
                    #(#deleg_attrs)*
                    #entity_attr
                    #gsig { #checks #gate #bind_prelude #(#pipe_prelude)* #body }
                }
            } else {
                let role_lit = LitStr::new(role_label, proc_macro2::Span::call_site());
                let succeeded = quote!(::core::result::Result::is_ok);
                let mut gsig = gsig;
                gsig.asyncness = Some(syn::token::Async(proc_macro2::Span::call_site()));
                quote! {
                    #(#deleg_attrs)*
                    #entity_attr
                    #gsig {
                        let __operation = ::nest_rs_graphql::GraphqlOperationContext::field(#gctx);
                        ::nest_rs_graphql::__private::run_operation(
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

        // The developer's method keeps its docs, lint levels and `#[cfg]`s.
        method.attrs.retain(|a| {
            a.path().is_ident("doc") || a.path().is_ident("allow") || a.path().is_ident("expect")
        });
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

    // Two walks over two token types; appending keeps keys and labels aligned.
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
    // `Guard::check_graphql` defaults to `Ok(())`: the bound makes a guard declare it.
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

/// Build a field resolver's `#[ComplexObject]` method, the parent as `&self`.
/// Owned args become GraphQL field arguments; `&`-reference args are injected.
fn field_method(
    self_ty: &Type,
    deleg_attrs: &[Attribute],
    sig: &Signature,
    guards: &[Path],
    force_guards: &[Path],
    field_label: &str,
) -> syn::Result<(Type, TokenStream2, Vec<Type>)> {
    let owned_sig = {
        let mut s = sig.clone();
        normalize_forwarded_args(s.inputs.iter_mut())?;
        s
    };
    let sig = &owned_sig;

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

    let mut gql_args: Vec<&FnArg> = Vec::new();
    let mut call_args: Vec<TokenStream2> = Vec::new();
    let mut dep_bindings: Vec<TokenStream2> = Vec::new();
    let mut injected_deps: Vec<Type> = Vec::new();
    for (arg, ident) in rest.iter().copied().zip(&rest_idents) {
        let FnArg::Typed(pt) = arg else { continue };
        // Before the injected-dep arm, which would ask the container for a `Context`.
        if ctx_ident_of(arg).is_some() {
            call_args.push(quote! { __ctx });
            continue;
        }
        if let Type::Reference(reference) = &*pt.ty {
            let dep_ty = &*reference.elem;
            let dep = format_ident!("__dep_{}", ident);
            if is_dataloader(dep_ty) {
                // Panics only if `GraphqlModule` was never imported.
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

    let checks = layered_resolver_chain(
        self_ty,
        guards,
        force_guards,
        &format_ident!("__ctx"),
        field_label,
        ChainSite::Field,
    );
    let role_lit = LitStr::new(FIELD_ROLE, proc_macro2::Span::call_site());
    let succeeded = quote!(::core::result::Result::is_ok);
    // By path: method lookup on a value tries a trait method taking `self` first.
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
            ::nest_rs_graphql::__private::run_operation(
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

fn field_parent_label(sig: &Signature) -> String {
    match sig.inputs.iter().nth(1) {
        Some(FnArg::Typed(typed)) => field_parent_key(match &*typed.ty {
            Type::Reference(reference) => &reference.elem,
            other => other,
        }),
        _ => String::new(),
    }
}

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

/// A method name as the schema serves it — a port of `Inflector`'s
/// `to_camel_case`, async-graphql's default (a new word after a digit too):
/// `get_2fa` is `get2Fa`, `userID` stays `userID`. Held against the derive by a test.
fn lower_camel(name: &str) -> String {
    let trimmed = name.trim_end_matches(|c: char| !c.is_alphanumeric());
    let mut camel = String::with_capacity(trimmed.len());
    let mut new_word = false;
    let mut last = ' ';
    let mut started = false;
    for c in trimmed.chars() {
        if !c.is_alphanumeric() {
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

/// State `field` as the name async-graphql serves a method under. It joins the
/// developer's `#[graphql(...)]`: async-graphql strips only the first.
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

fn is_dataloader(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp
        .path
        .segments
        .last()
        .is_some_and(|s| s.ident == "DataLoader"))
}

fn async_graphql_root() -> TokenStream2 {
    quote!(::nest_rs_graphql::async_graphql)
}

/// The same root as the string a `crate = ` argument takes, built from the
/// tokens: a stale string would parse and fall back to the call site's prelude.
fn async_graphql_root_str() -> String {
    async_graphql_root()
        .into_iter()
        .map(|t| t.to_string())
        .collect()
}

/// Which async-graphql root a set of operations becomes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RootKind {
    Query,
    Mutation,
    Subscription,
}

impl RootKind {
    fn variant(self) -> TokenStream2 {
        match self {
            Self::Query => quote!(Query),
            Self::Mutation => quote!(Mutation),
            Self::Subscription => quote!(Subscription),
        }
    }

    fn derive(self) -> Ident {
        match self {
            Self::Query | Self::Mutation => format_ident!("Object"),
            Self::Subscription => format_ident!("Subscription"),
        }
    }

    fn fake_type(self) -> Ident {
        match self {
            Self::Query | Self::Mutation => format_ident!("create_fake_output_type"),
            Self::Subscription => format_ident!("create_fake_subscription_type"),
        }
    }

    fn member(self) -> TokenStream2 {
        match self {
            Self::Query | Self::Mutation => quote!(Object),
            Self::Subscription => quote!(Subscription),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
            Self::Subscription => "subscription",
        }
    }
}

const ENTITY_ROLE: &str = "entity";

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

        // Without `crate = `, the derive falls back to the call site's bare `::async_graphql`.
        #[#root::#derive(crate = #root_str)]
        impl #obj {
            #(#methods)*
        }

        ::nest_rs_graphql::__private::inventory::submit! {
            ::nest_rs_graphql::__private::GraphqlResolverRegistration {
                kind: ::nest_rs_graphql::__private::GraphqlResolverKind::#variant,
                resolver_name: #resolver_name,
                resolver_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                entities: || ::std::vec![#(#entity_claims),*],
                type_info: |__r| __r.#fake_type::<#obj>(),
                build: |__c| ::nest_rs_graphql::__private::GraphqlRootMember::#member(
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
