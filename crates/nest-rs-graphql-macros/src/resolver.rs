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
    InjectableBody, build_injectable_body, forwarded_arg_idents, forwarded_idents,
    from_container_method, impl_self_ident, injected_keys_with_layers,
    injected_method_with_layers, layer_inject_keys,
};

pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
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
    let guard_specs = graphql_guard_specs(&guards);

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
    take_path_list(attrs, "use_guards")
}

/// `#[force_guards(...)]` — the Layer-System opt-in that lets a per-method
/// guard re-run even when the same `TypeId` is already in the global chain.
/// Same shape as `#[use_guards]`.
fn take_force_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    take_path_list(attrs, "force_guards")
}

fn take_path_list(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<Vec<Path>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident(ident)) {
        return Err(syn::Error::new_spanned(
            &attr,
            format!("at most one `#[{ident}(...)]` here; list every guard in it"),
        ));
    }
    Ok(attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

/// Extract and remove a flag attribute (no args, no parens) like `#[public]`.
/// Returns `true` when present (and removes it), `false` when absent.
fn take_flag_attr(attrs: &mut Vec<Attribute>, ident: &str) -> bool {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return false;
    };
    attrs.remove(pos);
    true
}

/// True when the method's return type's last path segment is `Result` — the
/// macro only emits the global guard chain (with its `?`-propagated
/// `async_graphql::Error`) on `Result`-returning queries/mutations. A bare-
/// return resolver is treated as `#[public]` by signature: it can't surface
/// an authn/authz failure, so the global chain stays off it.
fn sig_returns_result(sig: &Signature) -> bool {
    match &sig.output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => match &**ty {
            Type::Path(tp) => tp.path.segments.last().is_some_and(|s| s.ident == "Result"),
            _ => false,
        },
    }
}

/// The ident of a method's `&Context<'_>` parameter (matched on the last
/// path segment), so guard injection reuses it instead of adding a second.
fn ctx_param_ident(sig: &Signature) -> Option<Ident> {
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

/// Ensure the delegating signature has a `&Context` (async-graphql injects
/// every `&Context<'_>` param so an added one is not a schema argument).
fn ensure_ctx_param(sig: &Signature) -> (Signature, Ident) {
    if let Some(ident) = ctx_param_ident(sig) {
        return (sig.clone(), ident);
    }
    let ident = format_ident!("__guard_ctx");
    let mut sig = sig.clone();
    sig.inputs
        .push(parse_quote!(#ident: &::nest_rs_graphql::async_graphql::Context<'_>));
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
    let method_specs = graphql_guard_specs(method_guards);
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
            let __container = #ctx.data_unchecked::<::nest_rs_core::Container>();
            ::nest_rs_guards::run_layered_graphql_chain(
                #ctx,
                __container,
                &<#self_ty>::__nestrs_resolver_guard_specs(),
                &#method_specs,
                &#force_typeids,
                #label_lit,
            ).await?;
        }
    }
}

fn graphql_guard_specs(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec![] };
    }
    let entries = paths.iter().map(|p| {
        quote! {
            ::nest_rs_guards::dispatch::ScopedLayerSpec {
                type_id: ::core::any::TypeId::of::<#p>(),
                name: ::core::any::type_name::<#p>(),
                resolve: |__c| ::nest_rs_core::Container::get::<#p>(__c)
                    .map(|__arc| __arc as ::std::sync::Arc<dyn ::nest_rs_guards::Guard>),
            }
        }
    });
    quote! { ::std::vec![#(#entries),*] }
}

fn force_guard_typeids(paths: &[Path]) -> TokenStream2 {
    if paths.is_empty() {
        return quote! { ::std::vec![] };
    }
    let entries = paths
        .iter()
        .map(|p| quote! { ::core::any::TypeId::of::<#p>() });
    quote! { ::std::vec![#(#entries),*] }
}

/// `#[resolver]` on the impl: split `#[query]`/`#[mutation]` methods into
/// generated `#[Object]` roots and register them.
fn resolver_impl(mut item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();

    let base = match impl_self_ident(&self_ty, "#[resolver]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };

    // Module-gating uses `TypeId::of::<Self>()` so `Self` must be `'static`.
    // Reject generics here for a friendly span — otherwise the user sees a
    // deep-in-macro `T: 'static` failure on the inventory submission.
    if !item.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &item.generics,
            "`#[resolver] impl` must be on a concrete, `'static` self type — \
             generic and lifetime parameters are not supported (the resolver's \
             `TypeId` is its container key, which requires `'static`)",
        )
        .to_compile_error()
        .into();
    }

    // `#[use_guards(...)]` belongs on the struct (provider scope), uniform
    // with `#[controller]` and `#[gateway]`. Catch the legacy impl-block
    // placement here with a redirect message — the impl-form has no other
    // role for it (the struct-form parses and exposes it via
    // `__nestrs_resolver_guard_specs()`).
    if let Some(attr) = item.attrs.iter().find(|a| a.path().is_ident("use_guards")) {
        return syn::Error::new_spanned(
            attr,
            "put `#[use_guards(...)]` on the resolver's `struct`, not its `impl` block — \
             uniform with `#[controller]` and `#[gateway]`",
        )
        .to_compile_error()
        .into();
    }

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

        let method_guards = match take_use_guards(&mut method.attrs) {
            Ok(guards) => guards,
            Err(err) => return err.to_compile_error().into(),
        };
        let force_method_guards = match take_force_guards(&mut method.attrs) {
            Ok(paths) => paths,
            Err(err) => return err.to_compile_error().into(),
        };
        // Consume `#[public]` so it isn't emitted as an unknown attribute;
        // GraphQL guards that care can inspect a custom marker the dev
        // seeds via `ContextSeed` (the framework does not act on the flag).
        let _is_public = take_flag_attr(&mut method.attrs, "public");
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
            // Field resolvers gate per-row — they never replay the global
            // chain; only their own `#[use_guards]` apply.
            let field_label = format!("{}.{}", quote!(#self_ty), method_name);
            let (parent_ty, deleg, deps) = match field_method(
                &self_ty,
                &deleg_attrs,
                &sig,
                &method_guards,
                &force_method_guards,
                &field_label,
            ) {
                Ok(triple) => triple,
                Err(err) => return err.to_compile_error().into(),
            };
            field_dep_types.extend(deps);
            let key = quote!(#parent_ty).to_string();
            match field_groups
                .iter_mut()
                .find(|(ty, _)| quote!(#ty).to_string() == key)
            {
                Some((_, methods)) => methods.push(deleg),
                None => field_groups.push((parent_ty, vec![deleg])),
            }
        } else {
            let is_query = verb_attr.path().is_ident("query");
            let arg_idents = match forwarded_arg_idents(&sig) {
                Ok(idents) => idents,
                Err(err) => return err.to_compile_error().into(),
            };
            let call = if sig.asyncness.is_some() {
                quote! { self.0.#method_name(#(#arg_idents),*).await }
            } else {
                quote! { self.0.#method_name(#(#arg_idents),*) }
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
            let (gsig, gctx) = ensure_ctx_param(&sig);
            let checks = layered_resolver_chain(
                &self_ty,
                &method_guards,
                &force_method_guards,
                &gctx,
                &route_label,
                needs_global,
            );
            let delegating = quote! {
                #(#deleg_attrs)*
                #gsig { #checks #call }
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

    quote! {
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
    }
    .into()
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
                dep_bindings.push(quote! {
                    let #dep = __container.get::<#dep_ty>().expect(#msg);
                });
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
    quote! {
        #[allow(non_camel_case_types)]
        pub struct #obj(::std::sync::Arc<#self_ty>);

        #[::nest_rs_graphql::async_graphql::Object]
        impl #obj {
            #(#methods)*
        }

        ::nest_rs_graphql::inventory::submit! {
            ::nest_rs_graphql::ResolverRegistration {
                kind: ::nest_rs_graphql::ResolverKind::#kind,
                resolver_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                type_info: |__r| __r.create_fake_output_type::<#obj>(),
                build: |__c| ::std::boxed::Box::new(
                    #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                ) as ::std::boxed::Box<dyn ::nest_rs_graphql::ResolverObject>,
            }
        }
    }
}
