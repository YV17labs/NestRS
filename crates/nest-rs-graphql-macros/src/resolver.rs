//! `#[resolver]`: construction on a struct, operation orchestration on its
//! impl block.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, FnArg, Ident, ImplItem, Item, ItemImpl, ItemStruct, LitStr, Path, Signature, Token,
    Type, parse_macro_input, parse_quote,
};

use nest_rs_codegen::{
    InjectableBody, PipeWrapper, build_injectable_body, force_guard_typeids, forwarded_arg_idents,
    forwarded_idents, from_container_method, impl_self_ident, injected_keys_with_layers,
    injected_method_with_layers, layer_inject_keys, pipe_wrapper, reject_http_only_layers,
    scoped_specs, take_flag_attr, take_path_list,
};

pub(crate) fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[resolver] takes no arguments; tag methods with `#[query]` / `#[mutation]` / `#[field_resolver]`",
        )
        .to_compile_error()
        .into();
    }

    match parse_macro_input!(input as Item) {
        Item::Struct(item) => resolver_struct(item),
        Item::Impl(item) => resolver_impl(item),
        other => syn::Error::new_spanned(
            other,
            "#[resolver] applies to a struct (construction) or its impl block (query/mutation methods)",
        )
        .to_compile_error()
        .into(),
    }
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

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
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
    let injected_keys = injected_keys_with_layers(&dep_keys, guards.iter());
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));

    // Resolver-membership marker so the boot can require this resolver be
    // listed in a reachable module's `providers` (its schema presence is
    // unconditional via the registry). A generic resolver has no single
    // `TypeId` so it can't be a `providers` entry.
    let descriptor = if item.generics.params.is_empty() {
        quote! {
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_core::ResolverDescriptor {
                    resolver: || ::core::any::TypeId::of::<#name>(),
                    name: #name_str,
                }
            }
        }
    } else {
        quote!()
    };

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container

            #[doc(hidden)]
            pub fn __nestrs_injected() -> ::std::vec::Vec<::core::any::TypeId> {
                #injected_keys
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
    take_path_list(attrs, "use_guards", "guard")
}

/// `#[force_guards(...)]` — the Layer-System opt-in that lets a per-method
/// guard re-run even when the same `TypeId` is already in the global chain.
/// Same shape as `#[use_guards]`.
fn take_force_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    take_path_list(attrs, "force_guards", "guard")
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
    Bind(Path),
    IdArg(Ident),
}

impl syn::parse::Parse for AuthorizeArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(Ident) && input.peek2(Token![=]) {
            let name: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            if name == "bind" {
                Ok(AuthorizeArg::Bind(input.parse()?))
            } else if name == "id_arg" {
                Ok(AuthorizeArg::IdArg(input.parse()?))
            } else {
                Err(syn::Error::new_spanned(
                    name,
                    "unknown `#[authorize]` option — expected `bind = Service` or `id_arg = ident`",
                ))
            }
        } else {
            Ok(AuthorizeArg::Positional(input.parse()?))
        }
    }
}

/// Extract and remove a `#[authorize(...)]` attribute. At most one per method.
fn take_authorize(attrs: &mut Vec<Attribute>) -> syn::Result<Option<AuthorizeSpec>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("authorize")) else {
        return Ok(None);
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident("authorize")) {
        return Err(syn::Error::new_spanned(
            &attr,
            "at most one `#[authorize(...)]` per operation",
        ));
    }
    let args: Vec<AuthorizeArg> = attr
        .parse_args_with(Punctuated::<AuthorizeArg, Token![,]>::parse_terminated)?
        .into_iter()
        .collect();
    let shape_err = || {
        syn::Error::new_spanned(
            &attr,
            "expected `#[authorize(Action, Entity)]` — e.g. `#[authorize(Read, users::Entity)]`; \
             append `unmasked` to keep the class gate but mask the response yourself. \
             `bind = Service` (optionally `id_arg = ident`) binds the subject from an id \
             argument, and lets the entity be omitted (derived from `Service::Entity`): \
             `#[authorize(Update, bind = ArtworksService)]`",
        )
    };
    let mut positional: Vec<Path> = Vec::new();
    let mut bind: Option<Path> = None;
    let mut id_arg: Option<Ident> = None;
    for arg in args {
        match arg {
            AuthorizeArg::Positional(p) => positional.push(p),
            AuthorizeArg::Bind(p) => bind = Some(p),
            AuthorizeArg::IdArg(i) => id_arg = Some(i),
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

/// True when the method's return type's last path segment ends with `Result`
/// (`Result`, `GqlResult`, any `*Result` alias) — the macro only emits the
/// global guard chain (with its `?`-propagated `async_graphql::Error`) on
/// `Result`-returning queries/mutations. A bare-return resolver can't surface
/// an authn/authz failure, so the global chain stays off it (and the posture
/// check forces it to be `#[public]`). An alias that hides `Result` under an
/// unrelated name isn't recognised — spell the return type `Result` there.
fn sig_returns_result(sig: &Signature) -> bool {
    match &sig.output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => match &**ty {
            Type::Path(tp) => tp
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident.to_string().ends_with("Result")),
            _ => false,
        },
    }
}

/// The ident of a method's `&Context<'_>` parameter (matched on the last
/// path segment), so guard injection reuses it instead of adding a second.
pub(crate) fn ctx_param_ident(sig: &Signature) -> Option<Ident> {
    sig.inputs.iter().find_map(|arg| {
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
    })
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

/// Emit the unified Layer System chain for a resolver operation: global +
/// resolver-scope + per-method guards, deduped by `TypeId`. Resolver-scope
/// guards are read at runtime via `<Self>::__nestrs_resolver_guard_specs()`
/// — emitted by the struct-form `#[resolver]` macro, parallel to how
/// `#[controller]` exposes `__nestrs_controller_guard_specs()` for
/// `#[routes]` to consume. This is what makes the declaration site uniform:
/// the developer writes `#[use_guards(...)]` on the struct, same as for
/// HTTP controllers and WS gateways.
///
/// `needs_global = false` (a bare-return resolver that can't surface a
/// denial) AND no method/force guards skips the chain entirely. Resolver-
/// scope guards alone still trigger the chain because the struct may have
/// declared them.
fn layered_resolver_chain(
    self_ty: &Type,
    method_guards: &[Path],
    force_guards: &[Path],
    ctx: &Ident,
    route_label: &str,
    needs_global: bool,
) -> TokenStream2 {
    let label_lit = LitStr::new(route_label, proc_macro2::Span::call_site());
    let method_specs = scoped_specs(method_guards, quote!(dyn ::nest_rs_guards::Guard));
    let force_typeids = force_guard_typeids(force_guards);
    if !needs_global && method_guards.is_empty() && force_guards.is_empty() {
        // Bare-return resolver with no method/force guards. Bare-return
        // can't surface an `Err`, so emitting a chain that propagates `?`
        // would not compile; it also can't enforce any auth posture, so
        // skipping is honest. Resolver-scope guards on the struct only
        // run when the method returns `Result` — which is also where
        // auth/authz denials make sense semantically.
        return quote!();
    }
    quote! {
        {
            // Composed once per site against this container, then memoized —
            // the GraphQL analog of `RouteShaper`'s mount-time composition.
            static __NESTRS_GUARD_CHAIN: ::nest_rs_guards::GraphqlChainCell =
                ::nest_rs_guards::GraphqlChainCell::new();
            let __container = #ctx.data_unchecked::<::nest_rs_core::Container>();
            ::nest_rs_guards::run_layered_graphql_chain(
                #ctx,
                __container,
                &__NESTRS_GUARD_CHAIN,
                #label_lit,
                || ::nest_rs_guards::GraphqlChainSources {
                    resolver: <#self_ty>::__nestrs_resolver_guard_specs(),
                    method: #method_specs,
                    force: #force_typeids,
                },
            ).await?;
        }
    }
}

/// `#[resolver]` on the impl: split `#[query]`/`#[mutation]` methods into
/// generated `#[Object]` roots and register them.
fn resolver_impl(item: ItemImpl) -> TokenStream {
    match resolver_impl_inner(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The `#[resolver]`-on-impl expansion, returning `syn::Result<TokenStream2>`
/// so its gates are unit-testable without the `proc_macro` bridge —
/// `resolver_impl` is the thin `proc_macro::TokenStream` wrapper, the same
/// `entry`/`crud` split `#[crud]` uses. The mandatory-posture check below is
/// security-load-bearing: a `#[query]`/`#[mutation]` carrying neither
/// `#[authorize(...)]` nor `#[public]` must be a compile error, never an
/// ungated, unmasked operation.
fn resolver_impl_inner(mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let self_ty = item.self_ty.clone();

    let base = impl_self_ident(&self_ty, "#[resolver]")?;

    // Module-gating uses `TypeId::of::<Self>()` so `Self` must be `'static`.
    // Reject generics here for a friendly span — otherwise the user sees a
    // deep-in-macro `T: 'static` failure on the inventory submission.
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "`#[resolver] impl` must be on a concrete, `'static` self type — \
             generic and lifetime parameters are not supported (the resolver's \
             `TypeId` is its container key, which requires `'static`)",
        ));
    }

    // `#[use_guards(...)]` belongs on the struct (provider scope), uniform
    // with `#[controller]` and `#[gateway]`. Catch the legacy impl-block
    // placement here with a redirect message — the impl-form has no other
    // role for it (the struct-form parses and exposes it via
    // `__nestrs_resolver_guard_specs()`).
    if let Some(attr) = item.attrs.iter().find(|a| a.path().is_ident("use_guards")) {
        return Err(syn::Error::new_spanned(
            attr,
            "put `#[use_guards(...)]` on the resolver's `struct`, not its `impl` block — \
             uniform with `#[controller]` and `#[gateway]`",
        ));
    }
    reject_http_only_layers(&item.attrs, "GraphQL", "resolver")?;

    let query_obj = format_ident!("__{}Query", base);
    let mutation_obj = format_ident!("__{}Mutation", base);

    let mut query_methods: Vec<TokenStream2> = Vec::new();
    let mut mutation_methods: Vec<TokenStream2> = Vec::new();
    // async-graphql wants one `#[ComplexObject]` per parent type, so a
    // resolver's `#[field_resolver]` methods for the same parent merge into one impl.
    let mut field_groups: Vec<(Type, Vec<TokenStream2>)> = Vec::new();
    // Extra access-contract deps on top of the struct's `#[inject]` keys:
    // per-method guards + `#[field_resolver]` `&Service` injections.
    // Resolver-scope guards live in the struct's `__nestrs_injected()`
    // (parallel to `#[controller]` / `#[gateway]`).
    let mut all_guard_paths: Vec<Path> = Vec::new();
    let mut field_dep_types: Vec<Type> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let verb_idx = method.attrs.iter().position(|a| {
            a.path().is_ident("query")
                || a.path().is_ident("mutation")
                || a.path().is_ident("field_resolver")
        });
        let Some(idx) = verb_idx else { continue };

        let verb_attr = method.attrs.remove(idx);

        reject_http_only_layers(&method.attrs, "GraphQL", "resolver")?;
        let method_guards = take_use_guards(&mut method.attrs)?;
        let force_method_guards = take_force_guards(&mut method.attrs)?;
        // The operation's access posture: `#[authorize(Action, Entity)]`
        // (class gate + automatic response masking) or `#[public]`
        // (deliberately ungated). Exactly one is required on every
        // `#[query]`/`#[mutation]` — see the posture check below.
        let authorize_spec = take_authorize(&mut method.attrs)?;
        let is_public = take_flag_attr(&mut method.attrs, "public");
        all_guard_paths.extend(method_guards.iter().cloned());
        all_guard_paths.extend(force_method_guards.iter().cloned());
        // `#[field_resolver]` skips resolver-level guards: a field resolver
        // runs per-row, and the operation's auth posture is already enforced
        // by the operation guard plus the resolver-level guard on the root
        // query/mutation. Running it per row would just re-probe the
        // ability for every element. A `#[field_resolver]` needing its own
        // gate still binds `#[use_guards]` at the method level. The access
        // graph still sees the resolver-level dependency via `all_guard_paths`.
        let is_field = verb_attr.path().is_ident("field_resolver");

        // The delegating method keeps the signature and any remaining attrs
        // (`#[graphql(...)]` belongs there); the inherent method holds the body.
        let deleg_attrs = method.attrs.clone();
        let sig = method.sig.clone();
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
            // Field resolvers gate per-row — they never replay the global
            // chain; only their own `#[use_guards]` apply.
            let field_label = format!("{}.{}", quote!(#self_ty), method_name);
            let (parent_ty, deleg, deps) = field_method(
                &self_ty,
                &deleg_attrs,
                &sig,
                &method_guards,
                &force_method_guards,
                &field_label,
            )?;
            field_dep_types.extend(deps);
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
            // ungated and unmasked. `#[authorize]` needs a `Result` return so
            // the gate's denial (and a masking failure) can surface.
            match (&authorize_spec, is_public) {
                (Some(_), true) => {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "`#[authorize(...)]` and `#[public]` contradict — an operation is \
                         gated or public, not both",
                    ));
                }
                (Some(_), false) if !sig_returns_result(&method.sig) => {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "`#[authorize(...)]` needs a `Result` return type so a denial (and a \
                         masking failure) can surface as a GraphQL error; a bare-return \
                         operation can only be `#[public]`",
                    ));
                }
                (None, false) => {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "every `#[query]`/`#[mutation]` declares its access posture: \
                         `#[authorize(Action, Entity)]` (class-level gate + automatic response \
                         masking — e.g. `#[authorize(Read, users::Entity)]`) or `#[public]` \
                         (no `#[authorize]` gate and no response mask — `#[use_guards]` \
                         guards still run)",
                    ));
                }
                _ => {}
            }
            let is_query = verb_attr.path().is_ident("query");
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
            let call = if sig.asyncness.is_some() {
                quote! { self.0.#method_name(#(#call_args),*).await }
            } else {
                quote! { self.0.#method_name(#(#call_args),*) }
            };
            // Global guard chain runs on `Result`-returning queries/mutations
            // only (bare-return resolvers can't surface a denial). Local
            // `#[use_guards]` chain runs through the same chain helper.
            let needs_global = sig_returns_result(&sig);
            let route_label = format!(
                "{} {}",
                if is_query { "query" } else { "mutation" },
                method_name,
            );
            // Always emit the chain: even when the method declares no
            // method-scope guards, the struct may have declared
            // resolver-scope guards (read at runtime through
            // `__nestrs_resolver_guard_specs()`). Bare-return resolvers
            // can't surface a denial, so they still skip globals — the
            // chain helper's `run_layered_graphql_chain` is harmless when
            // every scope is empty.
            let (gsig, gctx) = ensure_ctx_param(&wrapper_sig);
            let checks = layered_resolver_chain(
                &self_ty,
                &method_guards,
                &force_method_guards,
                &gctx,
                &route_label,
                needs_global,
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
                    quote! {
                        match #call {
                            ::core::result::Result::Ok(__out) => ::core::result::Result::Ok(
                                ::nest_rs_authz::graphql::masked_value_for::<#action, #entity, _>(
                                    #gctx, __out,
                                )?,
                            ),
                            ::core::result::Result::Err(__err) =>
                                ::core::result::Result::Err(__err),
                        }
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
                    let #ident = #apply.map_err(|__e|
                        ::nest_rs_graphql::async_graphql::Error::new(__e.message().to_string()))?;
                }
            });
            let delegating = quote! {
                #(#deleg_attrs)*
                #gsig { #checks #gate #bind_prelude #(#pipe_prelude)* #body }
            };
            if is_query {
                query_methods.push(delegating);
            } else {
                mutation_methods.push(delegating);
            }
        }

        method.attrs.retain(|a| a.path().is_ident("doc"));
        for input in method.sig.inputs.iter_mut() {
            if let FnArg::Typed(pt) = input {
                pt.attrs.clear();
            }
        }
    }

    let query_block = root_object(&query_obj, &self_ty, &query_methods, quote!(Query));
    let mutation_block = root_object(&mutation_obj, &self_ty, &mutation_methods, quote!(Mutation));
    let field_blocks = field_groups.iter().map(|(parent_ty, methods)| {
        quote! {
            #[::nest_rs_graphql::async_graphql::ComplexObject]
            impl #parent_ty {
                #(#methods)*
            }
        }
    });

    // `Discoverable::injected` = struct `#[inject]` keys + operation guards +
    // `#[field_resolver]` deps. `register` is a no-op: the schema builds the resolver
    // from the assembled container at boot.
    let mut layer_keys = layer_inject_keys(all_guard_paths.iter());
    layer_keys.extend(layer_inject_keys(field_dep_types.iter()));
    let injected_method = injected_method_with_layers(&self_ty, &layer_keys);

    Ok(quote! {
        #item

        #query_block
        #mutation_block
        #(#field_blocks)*

        impl ::nest_rs_core::Discoverable for #self_ty {
            #injected_method

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
    let mut inputs = sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(_)) => {}
        _ => {
            return Err(syn::Error::new_spanned(
                sig,
                "#[field_resolver] method needs a `&self` receiver (services come from the resolver's `#[inject]` fields)",
            ));
        }
    }

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
                // Non-panicking on `Result`-returning resolvers (the common
                // case): a missing provider degrades to a named GraphQL
                // error, matching the `data_opt` pattern the relation
                // resolvers use. A bare-return resolver has no error channel,
                // so the named panic stays there — the access graph has
                // already validated the dep at boot either way.
                if sig_returns_result(sig) {
                    dep_bindings.push(quote! {
                        let #dep = __container.get::<#dep_ty>().ok_or_else(|| {
                            ::nest_rs_graphql::async_graphql::Error::new(#msg)
                        })?;
                    });
                } else {
                    dep_bindings.push(quote! {
                        let #dep = __container.get::<#dep_ty>().expect(#msg);
                    });
                }
                call_args.push(quote! { &#dep });
                injected_deps.push(dep_ty.clone());
            }
        } else {
            call_args.push(quote! { #ident });
            gql_args.push(arg);
        }
    }

    let asyncness = &sig.asyncness;
    let generics = &sig.generics;
    let where_clause = &sig.generics.where_clause;
    let output = &sig.output;
    let await_tok = if sig.asyncness.is_some() {
        quote!(.await)
    } else {
        quote!()
    };

    // `#[field_resolver]` never runs global guards (operation-level
    // enforcement already happened), so `needs_global` is `false`. The
    // chain helper still consults `<Self>::__nestrs_resolver_guard_specs()`
    // for resolver-scope guards declared on the struct — same uniform
    // mental model. `is_public` is irrelevant: there's no global chain to skip.
    let checks = layered_resolver_chain(
        self_ty,
        guards,
        force_guards,
        &format_ident!("__ctx"),
        field_label,
        false,
    );
    let method = quote! {
        #(#deleg_attrs)*
        #asyncness fn #method_name #generics (
            &self,
            __ctx: &::nest_rs_graphql::async_graphql::Context<'_>
            #(, #gql_args)*
        ) #output #where_clause {
            #checks
            let __container = __ctx.data_unchecked::<::nest_rs_core::Container>();
            #(#dep_bindings)*
            <#self_ty>::from_container(__container).#method_name(self #(, #call_args)*) #await_tok
        }
    };
    Ok((parent_ty, method, injected_deps))
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

fn root_object(
    obj: &Ident,
    self_ty: &Type,
    methods: &[TokenStream2],
    kind: TokenStream2,
) -> TokenStream2 {
    if methods.is_empty() {
        return quote!();
    }
    // Resolver struct name, logged beside each mounted operation at boot.
    let resolver_name = impl_self_ident(self_ty, "#[resolver]")
        .map(|i| i.to_string())
        .unwrap_or_else(|_| "resolver".to_string());
    let resolver_name = LitStr::new(&resolver_name, proc_macro2::Span::call_site());
    quote! {
        #[allow(non_camel_case_types)]
        pub struct #obj(::std::sync::Arc<#self_ty>);

        #[::nest_rs_graphql::async_graphql::Object]
        impl #obj {
            #(#methods)*
        }

        ::nest_rs_graphql::inventory::submit! {
            ::nest_rs_graphql::GraphqlResolverRegistration {
                kind: ::nest_rs_graphql::GraphqlResolverKind::#kind,
                resolver_name: #resolver_name,
                resolver_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                type_info: |__r| __r.create_fake_output_type::<#obj>(),
                build: |__c| ::std::boxed::Box::new(
                    #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                ) as ::std::boxed::Box<dyn ::nest_rs_graphql::GraphqlResolverObject>,
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

    // `#[authorize]` needs a `Result` return so a denial (or a masking failure)
    // can surface as a GraphQL error — a bare-return authorized op is rejected.
    #[test]
    fn authorize_on_bare_return_fails_to_expand() {
        let item: ItemImpl = parse_quote! {
            impl DemoResolver {
                #[query]
                #[authorize(::nest_rs_authz::Read, Thing)]
                async fn thing(&self) -> i32 {
                    0
                }
            }
        };
        let err = resolver_impl_inner(item)
            .expect_err("an #[authorize] op with a bare return must fail to expand");
        assert!(err.to_string().contains("Result"), "{}", err);
    }
}
