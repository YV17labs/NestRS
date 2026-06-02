//! `#[resolver]`: construction on a struct, operation orchestration on its impl
//! block. See the entry doc in `lib.rs` and the crate-level module docs.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, parse_quote, Attribute, FnArg, Ident, ImplItem, Item, ItemImpl, ItemStruct,
    Path, Signature, Token, Type,
};

use nestrs_codegen::{
    build_injectable_body, forwarded_arg_idents, forwarded_idents, from_container_method,
    impl_self_ident, injected_keys_expr, injected_method_with_layers, layer_inject_keys,
    InjectableBody,
};

/// `#[resolver]` entry: applies to a struct (construction) or its impl block
/// (query/mutation/field methods).
pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[resolver] takes no arguments; tag methods with `#[query]` / `#[mutation]`",
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

/// `#[resolver]` on the struct: construction only, like `#[injectable]`.
fn resolver_struct(mut item: ItemStruct) -> TokenStream {
    // Guards bind to the impl block (where the operations are), not the struct —
    // the impl-form macro can't see the struct's attributes. Catch the mistake.
    if let Some(attr) = item.attrs.iter().find(|a| a.path().is_ident("use_guards")) {
        return syn::Error::new_spanned(
            attr,
            "put `#[use_guards(...)]` on the resolver's `impl` block, not the struct",
        )
        .to_compile_error()
        .into();
    }

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let name_str = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The resolver's `#[inject]` keys, exposed for the impl-block macro to read
    // back into `Discoverable::injected` (extended there with the operation guards
    // and `#[field]` service dependencies the impl block declares) — the same
    // struct/impl split `#[controller]`/`#[routes]` use. See `access.rs`.
    let injected_keys = injected_keys_expr(&dep_keys);

    // Submit the resolver-membership marker so the boot can require this resolver
    // be listed in a reachable module's `providers` (its schema presence is
    // unconditional via the GraphQL registry). Skipped for a generic resolver,
    // which has no single `TypeId` and cannot be a `providers` entry anyway.
    let descriptor = if item.generics.params.is_empty() {
        quote! {
            ::nestrs_core::inventory::submit! {
                ::nestrs_core::ResolverDescriptor {
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
        }

        #descriptor
    }
    .into()
}

/// Extract and remove a `#[use_guards(GuardA, GuardB)]` attribute from an
/// attribute list, returning the guard paths (empty when absent). Like
/// `nestrs-http-macros`, the attribute is *consumed* — removed so it never reaches
/// the compiler as an unknown attribute. At most one per item.
fn take_use_guards(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Path>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("use_guards")) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident("use_guards")) {
        return Err(syn::Error::new_spanned(
            &attr,
            "at most one `#[use_guards(...)]` here; list every guard in it",
        ));
    }
    Ok(attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

/// The ident of a method's `&Context<'_>` parameter, if it declares one (matched
/// by the last path segment being `Context`). Lets guard injection reuse the
/// operation's own context rather than add a second one.
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

/// Ensure an operation's delegating signature exposes a `&Context` — reuse its own
/// if present, else append a dedicated `__guard_ctx` (async-graphql injects every
/// `&Context<'_>` param, so an added one is not a schema argument). Returns the
/// (possibly extended) signature and the context ident guards should read.
fn ensure_ctx_param(sig: &Signature) -> (Signature, Ident) {
    if let Some(ident) = ctx_param_ident(sig) {
        return (sig.clone(), ident);
    }
    let ident = format_ident!("__guard_ctx");
    let mut sig = sig.clone();
    sig.inputs
        .push(parse_quote!(#ident: &::nestrs_graphql::async_graphql::Context<'_>));
    (sig, ident)
}

/// Emit the guard checks that run before a resolver operation: resolve each guard
/// from the container in the context and run it, `?`-propagating a denial as the
/// operation's GraphQL error. `ctx` is the in-scope `&Context` ident.
fn guard_checks(guards: &[Path], ctx: &Ident) -> TokenStream2 {
    let checks = guards.iter().map(|g| {
        let msg = format!(
            "#[use_guards] resolver guard `{}` is not registered — add it to a module's providers",
            quote!(#g),
        );
        quote! {
            ::nestrs_graphql::ResolverGuard::check(
                &*::nestrs_core::Container::get::<#g>(
                    #ctx.data_unchecked::<::nestrs_core::Container>(),
                )
                .expect(#msg),
                #ctx,
            )
            .await?;
        }
    });
    quote! { #(#checks)* }
}

/// `#[resolver]` on the impl: split `#[query]`/`#[mutation]` methods into
/// generated `#[Object]` roots and register them.
fn resolver_impl(mut item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();

    let base = match impl_self_ident(&self_ty, "#[resolver]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };

    // Resolver-level guards: `#[use_guards(...)]` on the impl block runs before
    // every operation of this resolver (the `@UseGuards` on a `@Resolver` class
    // analog). It is an inert attribute consumed here — stripped from the block.
    // Per-operation guards (on a `#[query]`/`#[mutation]`/`#[field]`) stack inside.
    let resolver_guards = match take_use_guards(&mut item.attrs) {
        Ok(guards) => guards,
        Err(err) => return err.to_compile_error().into(),
    };

    let query_obj = format_ident!("__{}Query", base);
    let mutation_obj = format_ident!("__{}Mutation", base);

    let mut query_methods: Vec<TokenStream2> = Vec::new();
    let mut mutation_methods: Vec<TokenStream2> = Vec::new();
    // Field resolvers grouped by the parent type they extend: async-graphql
    // wants one `#[ComplexObject]` per type, so a resolver's `#[field]` methods
    // for the same parent are merged into a single emitted impl.
    let mut field_groups: Vec<(Type, Vec<TokenStream2>)> = Vec::new();
    // Container-resolved dependencies the access contract must see, on top of the
    // struct's `#[inject]` fields (already in `__nestrs_injected`): every
    // operation guard (resolver- + method-level) and every `#[field]` `&Service`.
    let mut all_guard_paths: Vec<Path> = resolver_guards.clone();
    let mut field_dep_types: Vec<Type> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let verb_idx = method.attrs.iter().position(|a| {
            a.path().is_ident("query")
                || a.path().is_ident("mutation")
                || a.path().is_ident("field")
        });
        let Some(idx) = verb_idx else { continue };

        let verb_attr = method.attrs.remove(idx);

        // Per-operation guards (`#[use_guards(...)]` on this method), consumed here
        // so they leave neither the delegating attrs nor the inherent method. They
        // stack inside the resolver-level guards (resolver-level run first).
        let method_guards = match take_use_guards(&mut method.attrs) {
            Ok(guards) => guards,
            Err(err) => return err.to_compile_error().into(),
        };
        all_guard_paths.extend(method_guards.iter().cloned());
        let op_guards: Vec<Path> = resolver_guards
            .iter()
            .cloned()
            .chain(method_guards)
            .collect();

        // The delegating method keeps the (cleaned) signature and any remaining
        // attrs — async-graphql's `#[graphql(...)]` belongs there. The inherent
        // method, kept clean, holds the real body.
        let deleg_attrs = method.attrs.clone();
        let sig = method.sig.clone();
        let method_name = method.sig.ident.clone();

        if verb_attr.path().is_ident("field") {
            let (parent_ty, deleg, deps) =
                match field_method(&self_ty, &deleg_attrs, &sig, &op_guards) {
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
            // With guards, run them before delegating. They need the context (for
            // the container + the seeded principal/ability); reuse the method's own
            // `&Context` param when it has one, else inject a dedicated one. A
            // guard's `Err` short-circuits via `?`, so a guarded op returns a
            // `Result`.
            let delegating = if op_guards.is_empty() {
                quote! {
                    #(#deleg_attrs)*
                    #sig { #call }
                }
            } else {
                let (gsig, gctx) = ensure_ctx_param(&sig);
                let checks = guard_checks(&op_guards, &gctx);
                quote! {
                    #(#deleg_attrs)*
                    #gsig { #checks #call }
                }
            };
            if is_query {
                query_methods.push(delegating);
            } else {
                mutation_methods.push(delegating);
            }
        }

        // Clean the inherent method: keep only doc comments, drop arg attrs.
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
            #[::nestrs_graphql::async_graphql::ComplexObject]
            impl #parent_ty {
                #(#methods)*
            }
        }
    });

    // `Discoverable::injected` = the struct's `#[inject]` keys (from
    // `__nestrs_injected`, emitted by the struct macro) extended with the
    // operation guards and `#[field]` `&Service` dependencies gathered here, so a
    // resolver listed in `providers = [...]` is governed by the access contract
    // exactly like a controller. `register` is a no-op: the schema builds the
    // resolver from the assembled container at boot, it registers nothing.
    let mut layer_keys = layer_inject_keys(all_guard_paths.iter());
    layer_keys.extend(layer_inject_keys(field_dep_types.iter()));
    let injected_method = injected_method_with_layers(&self_ty, &layer_keys);

    quote! {
        #item

        #query_block
        #mutation_block
        #(#field_blocks)*

        impl ::nestrs_core::Discoverable for #self_ty {
            #injected_method

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder
            }
        }
    }
    .into()
}

/// Build a field resolver's `#[ComplexObject]` method from the inherent method's
/// signature. The inherent method's first value argument is the parent object
/// (`parent: &ParentType`); the generated method takes the parent as its
/// `&self`, builds the resolver from the container, and delegates. Of the
/// remaining arguments, owned ones become GraphQL field arguments while
/// `&`-reference ones are injected dependencies — a `&Service` from the
/// container, a `&DataLoader<…>` from the request context — so they never leak
/// into the schema. Returns the parent type so the caller can group methods by it.
fn field_method(
    self_ty: &Type,
    deleg_attrs: &[Attribute],
    sig: &Signature,
    guards: &[Path],
) -> syn::Result<(Type, TokenStream2, Vec<Type>)> {
    let mut inputs = sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(_)) => {}
        _ => {
            return Err(syn::Error::new_spanned(
                sig,
                "#[field] method needs a `&self` receiver (services come from the resolver's `#[inject]` fields)",
            ))
        }
    }

    let parent = inputs.next().ok_or_else(|| {
        syn::Error::new_spanned(
            sig,
            "#[field] method needs a parent argument `parent: &ParentType` — the object being resolved",
        )
    })?;
    let FnArg::Typed(parent) = parent else {
        return Err(syn::Error::new_spanned(
            parent,
            "#[field] parent argument must be typed",
        ));
    };
    let Type::Reference(parent_ref) = &*parent.ty else {
        return Err(syn::Error::new_spanned(
            &parent.ty,
            "#[field] parent argument must be a reference `&ParentType`",
        ));
    };
    let parent_ty = (*parent_ref.elem).clone();

    let rest: Vec<&FnArg> = inputs.collect();
    let rest_idents = forwarded_idents(rest.iter().copied())?;

    let method_name = &sig.ident;

    // Split the post-parent arguments: an owned one is a GraphQL field argument;
    // a `&`-reference one is an injected dependency, unambiguous since a `&T` is
    // never a GraphQL `InputType`. A `&DataLoader<…>` injects request-scoped from
    // the context; any other `&service` is a singleton from the container.
    let mut gql_args: Vec<&FnArg> = Vec::new();
    let mut call_args: Vec<TokenStream2> = Vec::new();
    let mut dep_bindings: Vec<TokenStream2> = Vec::new();
    // The container-resolved `&Service` dependency types (dataloaders excluded —
    // those are request-scoped, read from the context), reported up so the impl
    // macro folds them into `Discoverable::injected` for the access contract.
    let mut injected_deps: Vec<Type> = Vec::new();
    for (arg, ident) in rest.iter().copied().zip(&rest_idents) {
        let FnArg::Typed(pt) = arg else { continue };
        if let Type::Reference(reference) = &*pt.ty {
            let dep_ty = &*reference.elem;
            let dep = format_ident!("__dep_{}", ident);
            if is_dataloader(dep_ty) {
                // `data_unchecked` hands back the `&DataLoader<…>` the extension
                // seeded for this request; it panics only if `GraphqlModule` (and
                // thus the extension) was never imported.
                dep_bindings.push(quote! {
                    let #dep = __ctx.data_unchecked::<#dep_ty>();
                });
                call_args.push(quote! { #dep });
            } else {
                let msg = format!(
                    "#[field] `{}`: no provider registered for `{}`",
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

    // The generated method always has `__ctx`, so guards run against it before any
    // dependency is resolved or the body delegates.
    let checks = guard_checks(guards, &format_ident!("__ctx"));
    let method = quote! {
        #(#deleg_attrs)*
        #asyncness fn #method_name #generics (
            &self,
            __ctx: &::nestrs_graphql::async_graphql::Context<'_>
            #(, #gql_args)*
        ) #output #where_clause {
            #checks
            let __container = __ctx.data_unchecked::<::nestrs_core::Container>();
            #(#dep_bindings)*
            <#self_ty>::from_container(__container).#method_name(self #(, #call_args)*) #await_tok
        }
    };
    Ok((parent_ty, method, injected_deps))
}

/// Whether a `#[field]` dependency type is a `DataLoader<…>` (matched by its
/// final path segment, so both `DataLoader<L>` and the fully-qualified
/// `async_graphql::dataloader::DataLoader<L>` are recognised). DataLoaders are
/// request-scoped — read from the request context — while every other injected
/// dependency is a container singleton.
fn is_dataloader(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp
        .path
        .segments
        .last()
        .is_some_and(|s| s.ident == "DataLoader"))
}

/// Emit one generated `#[Object]` root + its registry submission, or nothing
/// when the resolver has no methods of that kind.
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

        #[::nestrs_graphql::async_graphql::Object]
        impl #obj {
            #(#methods)*
        }

        ::nestrs_graphql::inventory::submit! {
            ::nestrs_graphql::ResolverRegistration {
                kind: ::nestrs_graphql::ResolverKind::#kind,
                // The resolver struct's `TypeId` — its container key, what the
                // boot's reachable-provider set indexes — so module-gating can
                // filter this entry out of an app that does not import the
                // resolver's module.
                resolver_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                type_info: |__r| __r.create_fake_output_type::<#obj>(),
                build: |__c| ::std::boxed::Box::new(
                    #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                ) as ::std::boxed::Box<dyn ::nestrs_graphql::ResolverObject>,
            }
        }
    }
}
