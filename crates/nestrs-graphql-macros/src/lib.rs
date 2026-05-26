//! GraphQL decorator macros, re-exported by `nestrs-graphql`. The generated
//! code uses absolute paths (`::nestrs_graphql::*`, `::std::sync::Arc`), so this
//! crate does not depend on them — they resolve at the call site.
//!
//! Mirrors the HTTP `#[controller]`/`#[routes]` split: `#[resolver]` on a struct
//! handles construction (DI); `#[resolver]` on its impl block orchestrates the
//! method-level `#[query]`/`#[mutation]`/`#[field]` verbs.
//!
//! `#[field]` is the field-resolver verb (NestJS's `@ResolveField`): it adds a
//! computed/related field to an object type. Its parameters mirror NestJS's
//! `@Parent`/`@Args`/`@Loader`: the first, `parent: &ParentType`, is the
//! resolved object; owned parameters are GraphQL arguments; `&`-reference
//! parameters are injected dependencies — a `&Service` from the container, a
//! request-scoped `&DataLoader<…>` from the request context.
//! It lowers to async-graphql's `#[ComplexObject]`; see `field_method`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, Attribute, FnArg, GenericArgument, Ident, ImplItem, Item, ItemImpl,
    ItemStruct, PathArguments, ReturnType, Signature, Type,
};

use nestrs_codegen::{
    build_injectable_body, forwarded_arg_idents, forwarded_idents, from_container_method,
    impl_self_ident, InjectableBody,
};

/// Mark a GraphQL resolver.
///
/// Applied in two places, like `#[controller]` + `#[routes]`:
///
/// - **On the struct** — builds it from the container (`#[inject]` fields
///   resolved, others default), emitting `from_container`. The resolver is not
///   a provider; it is constructed at schema-build time.
/// - **On its impl block** — each method tagged `#[query]` or `#[mutation]` is
///   split into a generated `#[Object]` root (`__<Name>Query` /
///   `__<Name>Mutation`) that delegates back to the inherent method, and is
///   submitted to the link-time registry. The schema composes itself from that
///   registry (see `nestrs_graphql::build_schema`) — there is no central list.
///   Each method tagged `#[field]` instead becomes a field resolver on its
///   `parent: &ParentType` argument's type, emitted as a `#[ComplexObject]`
///   impl that delegates to the inherent method (see the module docs).
#[proc_macro_attribute]
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

/// Turn a data-layer impl block into batched DataLoaders — one per method.
///
/// Each method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (or
/// `Result<HashMap<K, V>, E>`) generates a hidden `Loader` named `<Owner><Name>`
/// (e.g. `UsersServiceByName`) wrapping `Arc<Owner>` and delegating to the
/// method, and submits a `LoaderRegistration` to the link-time registry — no
/// `#[module(providers = [...])]` entry. The loader is **request-scoped**: a
/// schema extension (installed by `GraphqlModule`) rebuilds it from the fully
/// assembled container at the start of each request and seeds it into the
/// GraphQL context, where a `#[field]` reads it as `&DataLoader<UsersServiceByName>`.
/// Concurrent field resolutions within one request collapse into a single
/// `load`, killing the N+1; the per-request instance keeps requests isolated and
/// makes `GraphqlModule`'s import order irrelevant (the loader is built when a
/// request arrives, never at registration time).
///
/// The batch query lives where the future ORM query will: on the service. The
/// spawner is `tokio::spawn`; nestrs apps already run on Tokio.
#[proc_macro_attribute]
pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(&args, "#[dataloader] takes no arguments")
            .to_compile_error()
            .into();
    }

    match parse_macro_input!(input as Item) {
        Item::Impl(item) => dataloader_impl(item),
        other => syn::Error::new_spanned(
            other,
            "#[dataloader] applies to a data-layer impl block; each method becomes a batched DataLoader",
        )
        .to_compile_error()
        .into(),
    }
}

/// `#[dataloader]` on an impl: one generated `Loader` per method.
fn dataloader_impl(item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();
    let base = match impl_self_ident(&self_ty, "#[dataloader]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };

    let mut loaders: Vec<TokenStream2> = Vec::new();
    for impl_item in &item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        match dataloader_for_method(&self_ty, &base, &method.sig) {
            Ok(loader) => loaders.push(loader),
            Err(err) => return err.to_compile_error().into(),
        }
    }

    quote! {
        #item

        #(#loaders)*
    }
    .into()
}

/// Generate one loader (struct + `Loader` impl + registry submission) from a
/// batch method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (the return
/// may be wrapped in `Result<_, E>`; a bare map loads infallibly).
fn dataloader_for_method(
    self_ty: &Type,
    base: &Ident,
    sig: &Signature,
) -> syn::Result<TokenStream2> {
    let key_ty = loader_key_type(sig)?;
    let (value_ty, error_ty) = loader_value_and_error(&sig.output)?;
    let method_name = &sig.ident;
    let loader_name = format_ident!("{}{}", base, pascal_case(method_name));
    let missing = format!(
        "{loader_name}: no provider registered for `{}`",
        quote!(#self_ty)
    );

    let call = if sig.asyncness.is_some() {
        quote! { self.0.#method_name(__keys).await }
    } else {
        quote! { self.0.#method_name(__keys) }
    };
    let (error_ty, load_body) = match error_ty {
        Some(err) => (quote!(#err), call),
        None => (
            quote!(::std::convert::Infallible),
            quote! { ::std::result::Result::Ok(#call) },
        ),
    };

    Ok(quote! {
        pub struct #loader_name(::std::sync::Arc<#self_ty>);

        impl #loader_name {
            fn from_container(container: &::nestrs_core::Container) -> Self {
                Self(container.get::<#self_ty>().expect(#missing))
            }
        }

        impl ::nestrs_graphql::async_graphql::dataloader::Loader<#key_ty> for #loader_name {
            type Value = #value_ty;
            type Error = #error_ty;

            async fn load(
                &self,
                __keys: &[#key_ty],
            ) -> ::std::result::Result<
                ::std::collections::HashMap<#key_ty, #value_ty>,
                Self::Error,
            > {
                #load_body
            }
        }

        ::nestrs_graphql::inventory::submit! {
            ::nestrs_graphql::LoaderRegistration {
                // Request-scoped: a fresh loader built per request from the fully
                // assembled container (not at module registration — which is what
                // makes import order irrelevant) and seeded into the request context.
                seed: |__container, __request| {
                    let __loader = <#loader_name>::from_container(__container);
                    __request.data(
                        ::nestrs_graphql::async_graphql::dataloader::DataLoader::new(
                            __loader,
                            ::tokio::spawn,
                        ),
                    )
                },
            }
        }
    })
}

/// `snake_case` → `PascalCase`, for naming a method's generated loader.
fn pascal_case(ident: &Ident) -> Ident {
    let mut out = String::new();
    let mut upper = true;
    for ch in ident.to_string().chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    Ident::new(&out, ident.span())
}

/// The `K` in a batch method's `keys: &[K]` argument (after `&self`).
fn loader_key_type(sig: &Signature) -> syn::Result<Type> {
    let mut inputs = sig.inputs.iter();
    if !matches!(inputs.next(), Some(FnArg::Receiver(_))) {
        return Err(syn::Error::new_spanned(
            sig,
            "#[dataloader] method needs a `&self` receiver",
        ));
    }
    let keys = inputs.next().ok_or_else(|| {
        syn::Error::new_spanned(sig, "#[dataloader] method needs a `keys: &[K]` argument")
    })?;
    let FnArg::Typed(pat) = keys else {
        return Err(syn::Error::new_spanned(
            keys,
            "#[dataloader] keys argument must be typed",
        ));
    };
    let Type::Reference(reference) = &*pat.ty else {
        return Err(syn::Error::new_spanned(
            &pat.ty,
            "#[dataloader] keys argument must be `&[K]`",
        ));
    };
    let Type::Slice(slice) = &*reference.elem else {
        return Err(syn::Error::new_spanned(
            &pat.ty,
            "#[dataloader] keys argument must be a slice `&[K]`",
        ));
    };
    Ok((*slice.elem).clone())
}

/// The value type `V` (and optional error `E`) of a batch method returning
/// `HashMap<K, V>` or `Result<HashMap<K, V>, E>`.
fn loader_value_and_error(output: &ReturnType) -> syn::Result<(Type, Option<Type>)> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "#[dataloader] method must return `HashMap<K, V>` or `Result<HashMap<K, V>, E>`",
        ));
    };
    match generic_args(ty, "Result") {
        Some(args) if args.len() == 2 => Ok((hashmap_value(&args[0])?, Some(args[1].clone()))),
        _ => Ok((hashmap_value(ty)?, None)),
    }
}

/// The value type of a `HashMap<K, V>` (its second type argument).
fn hashmap_value(ty: &Type) -> syn::Result<Type> {
    match generic_args(ty, "HashMap") {
        Some(args) if args.len() == 2 => Ok(args[1].clone()),
        _ => Err(syn::Error::new_spanned(
            ty,
            "#[dataloader] method must return a `HashMap<K, V>` (optionally in `Result<_, E>`)",
        )),
    }
}

/// The angle-bracketed type arguments of `ty` when its last path segment is
/// `expected` (e.g. `Result`, `HashMap`).
fn generic_args(ty: &Type, expected: &str) -> Option<Vec<Type>> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != expected {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    Some(
        args.args
            .iter()
            .filter_map(|arg| match arg {
                GenericArgument::Type(t) => Some(t.clone()),
                _ => None,
            })
            .collect(),
    )
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

/// `#[resolver]` on the struct: construction only, like `#[injectable]`.
fn resolver_struct(mut item: ItemStruct) -> TokenStream {
    let InjectableBody { ctor, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }
    }
    .into()
}

/// `#[resolver]` on the impl: split `#[query]`/`#[mutation]` methods into
/// generated `#[Object]` roots and register them.
fn resolver_impl(mut item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();

    let base = match impl_self_ident(&self_ty, "#[resolver]") {
        Ok(base) => base,
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

        // The delegating method keeps the (cleaned) signature and any remaining
        // attrs — async-graphql's `#[graphql(...)]` belongs there. The inherent
        // method, kept clean, holds the real body.
        let deleg_attrs = method.attrs.clone();
        let sig = method.sig.clone();
        let method_name = method.sig.ident.clone();

        if verb_attr.path().is_ident("field") {
            let (parent_ty, deleg) = match field_method(&self_ty, &deleg_attrs, &sig) {
                Ok(pair) => pair,
                Err(err) => return err.to_compile_error().into(),
            };
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
            let delegating = quote! {
                #(#deleg_attrs)*
                #sig { #call }
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

    quote! {
        #item

        #query_block
        #mutation_block
        #(#field_blocks)*
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
) -> syn::Result<(Type, TokenStream2)> {
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

    let method = quote! {
        #(#deleg_attrs)*
        #asyncness fn #method_name #generics (
            &self,
            __ctx: &::nestrs_graphql::async_graphql::Context<'_>
            #(, #gql_args)*
        ) #output #where_clause {
            let __container = __ctx.data_unchecked::<::nestrs_core::Container>();
            #(#dep_bindings)*
            <#self_ty>::from_container(__container).#method_name(self #(, #call_args)*) #await_tok
        }
    };
    Ok((parent_ty, method))
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
                type_info: |__r| __r.create_fake_output_type::<#obj>(),
                build: |__c| ::std::boxed::Box::new(
                    #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                ) as ::std::boxed::Box<dyn ::nestrs_graphql::ResolverObject>,
            }
        }
    }
}
