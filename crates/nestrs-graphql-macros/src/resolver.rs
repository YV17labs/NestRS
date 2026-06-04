//! `#[resolver]`: construction on a struct, operation orchestration on its
//! impl block.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, FnArg, Ident, ImplItem, Item, ItemImpl, ItemStruct, Path, Signature, Token, Type,
    parse_macro_input, parse_quote,
};

use nestrs_codegen::{
    InjectableBody, build_injectable_body, forwarded_arg_idents, forwarded_idents,
    from_container_method, impl_self_ident, injected_keys_expr, injected_method_with_layers,
    layer_inject_keys,
};

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
    // Guards bind to the impl block, not the struct — catch the mistake here
    // because the impl-form macro can't see the struct's attributes.
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
    // The struct's `#[inject]` keys, exposed for the impl-block macro to fold
    // into `Discoverable::injected` together with operation guards and
    // `#[field]` `&Service` deps. Same struct/impl split as
    // `#[controller]`/`#[routes]`.
    let injected_keys = injected_keys_expr(&dep_keys);

    // Resolver-membership marker so the boot can require this resolver be
    // listed in a reachable module's `providers` (its schema presence is
    // unconditional via the registry). A generic resolver has no single
    // `TypeId` so it can't be a `providers` entry.
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

/// Extract and remove a `#[use_guards(...)]` attribute, returning its paths.
/// The attribute is consumed so it never reaches the compiler as an unknown
/// attribute. At most one per item.
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
        .push(parse_quote!(#ident: &::nestrs_graphql::async_graphql::Context<'_>));
    (sig, ident)
}

/// Emit guard checks before a resolver operation. A missing provider should
/// have been caught by the access graph at boot; a slip-through converts to a
/// GraphQL error rather than panicking the worker.
fn guard_checks(guards: &[Path], ctx: &Ident) -> TokenStream2 {
    let checks = guards.iter().map(|g| {
        let msg = format!(
            "#[use_guards] resolver guard `{}` is not registered — add it to a module's providers",
            quote!(#g),
        );
        quote! {
            {
                let __guard = match ::nestrs_core::Container::get::<#g>(
                    #ctx.data_unchecked::<::nestrs_core::Container>(),
                ) {
                    ::core::option::Option::Some(__g) => __g,
                    ::core::option::Option::None => {
                        ::tracing::error!(
                            target: "nestrs::graphql",
                            guard = stringify!(#g),
                            "resolver guard is missing from the container — boot wiring bug",
                        );
                        return ::core::result::Result::Err(
                            ::nestrs_graphql::async_graphql::Error::new(#msg),
                        );
                    }
                };
                ::nestrs_graphql::ResolverGuard::check(&*__guard, #ctx).await?;
            }
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

    // `#[use_guards(...)]` on the impl block — `@UseGuards` on a `@Resolver`
    // class analog. Per-method guards stack inside.
    let resolver_guards = match take_use_guards(&mut item.attrs) {
        Ok(guards) => guards,
        Err(err) => return err.to_compile_error().into(),
    };

    let query_obj = format_ident!("__{}Query", base);
    let mutation_obj = format_ident!("__{}Mutation", base);

    let mut query_methods: Vec<TokenStream2> = Vec::new();
    let mut mutation_methods: Vec<TokenStream2> = Vec::new();
    // async-graphql wants one `#[ComplexObject]` per parent type, so a
    // resolver's `#[field]` methods for the same parent merge into one impl.
    let mut field_groups: Vec<(Type, Vec<TokenStream2>)> = Vec::new();
    // Extra access-contract deps on top of the struct's `#[inject]` keys:
    // operation guards + `#[field]` `&Service` injections.
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

        let method_guards = match take_use_guards(&mut method.attrs) {
            Ok(guards) => guards,
            Err(err) => return err.to_compile_error().into(),
        };
        all_guard_paths.extend(method_guards.iter().cloned());
        // `#[field]` skips resolver-level guards: a field resolver runs
        // per-row, and the operation's auth posture is already enforced by
        // the operation guard plus the resolver-level guard on the root
        // query/mutation. Running it per row would just re-probe the
        // ability for every element. A `#[field]` needing its own gate
        // still binds `#[use_guards]` at the method level. The access graph
        // still sees the resolver-level dependency via `all_guard_paths`.
        let is_field = verb_attr.path().is_ident("field");
        let op_guards: Vec<Path> = if is_field {
            method_guards
        } else {
            resolver_guards
                .iter()
                .cloned()
                .chain(method_guards)
                .collect()
        };

        // The delegating method keeps the signature and any remaining attrs
        // (`#[graphql(...)]` belongs there); the inherent method holds the body.
        let deleg_attrs = method.attrs.clone();
        let sig = method.sig.clone();
        let method_name = method.sig.ident.clone();

        if is_field {
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

    // `Discoverable::injected` = struct `#[inject]` keys + operation guards +
    // `#[field]` deps. `register` is a no-op: the schema builds the resolver
    // from the assembled container at boot.
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
) -> syn::Result<(Type, TokenStream2, Vec<Type>)> {
    let mut inputs = sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(_)) => {}
        _ => {
            return Err(syn::Error::new_spanned(
                sig,
                "#[field] method needs a `&self` receiver (services come from the resolver's `#[inject]` fields)",
            ));
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

        #[::nestrs_graphql::async_graphql::Object]
        impl #obj {
            #(#methods)*
        }

        ::nestrs_graphql::inventory::submit! {
            ::nestrs_graphql::ResolverRegistration {
                kind: ::nestrs_graphql::ResolverKind::#kind,
                resolver_type_id: || ::core::any::TypeId::of::<#self_ty>(),
                type_info: |__r| __r.create_fake_output_type::<#obj>(),
                build: |__c| ::std::boxed::Box::new(
                    #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                ) as ::std::boxed::Box<dyn ::nestrs_graphql::ResolverObject>,
            }
        }
    }
}
