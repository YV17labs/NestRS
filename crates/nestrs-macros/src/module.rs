use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{bracketed, parse_macro_input, Expr, Ident, ItemStruct, Path, Token, Type};

pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ModuleArgs);
    let item = parse_macro_input!(input as ItemStruct);
    let name = item.ident.clone();
    let name_str = name.to_string();

    let import_calls = args.imports.iter().map(|import| match import {
        // Bare type path → static `Module`.
        Expr::Path(p) => {
            let path = &p.path;
            quote! { builder = <#path as ::nestrs_core::Module>::register(builder); }
        }
        // Anything else → `DynamicModule` value (e.g. `Module::for_root(opts)`).
        other => {
            quote! { builder = ::nestrs_core::DynamicModule::register(#other, builder); }
        }
    });

    // Collect phase only queues async factories; providers untouched here.
    let collect_calls = args.imports.iter().map(|import| match import {
        Expr::Path(p) => {
            let path = &p.path;
            quote! { builder = <#path as ::nestrs_core::Module>::collect(builder); }
        }
        other => {
            quote! { builder = ::nestrs_core::DynamicModule::collect(&(#other), builder); }
        }
    });

    // Access-graph descriptor submitted to the link-time registry. Only
    // statically-typed imports are recorded — a dynamic `for_root(...)`
    // contributes only global infrastructure, never an injectable.
    let import_type_ids = args.imports.iter().filter_map(|import| match import {
        Expr::Path(p) => {
            let path = &p.path;
            Some(quote! { || ::std::any::TypeId::of::<#path>() })
        }
        _ => None,
    });
    let provider_descriptors = args.providers.iter().map(|binding| match binding {
        ProviderBinding::Concrete(p) => {
            let name_lit = path_tail(p);
            quote! {
                ::nestrs_core::ProviderDescriptor {
                    name: #name_lit,
                    provides: || ::std::any::TypeId::of::<#p>(),
                    injects: <#p as ::nestrs_core::Discoverable>::injected,
                }
            }
        }
        ProviderBinding::Dyn { provider, trait_ty } => {
            let name_lit = format!("dyn {}", path_tail_of_type(trait_ty));
            quote! {
                ::nestrs_core::ProviderDescriptor {
                    name: #name_lit,
                    provides: || ::std::any::TypeId::of::<::std::sync::Arc<#trait_ty>>(),
                    injects: <#provider as ::nestrs_core::Discoverable>::injected,
                }
            }
        }
    });
    let descriptor_submission = quote! {
        ::nestrs_core::inventory::submit! {
            ::nestrs_core::ModuleDescriptor {
                module: || ::std::any::TypeId::of::<#name>(),
                name: #name_str,
                imports: &[ #(#import_type_ids),* ],
                providers: &[ #(#provider_descriptors),* ],
            }
        }
    };

    let body = if args.providers.is_empty() {
        quote! {
            #(#import_calls)*
            builder
        }
    } else {
        let count = proc_macro2::Literal::usize_unsuffixed(args.providers.len());
        // Three token streams per provider: hot register attempt, its provided
        // key, and a stall-time classification of why it is still pending.
        let parts: Vec<(
            proc_macro2::TokenStream,
            proc_macro2::TokenStream,
            proc_macro2::TokenStream,
        )> = args
            .providers
            .iter()
            .enumerate()
            .map(|(i, binding)| {
                let idx = proc_macro2::Literal::usize_unsuffixed(i);
                let (provider, name_lit, provided_key, register_action) = match binding {
                    ProviderBinding::Concrete(p) => (
                        p,
                        path_tail(p),
                        quote! { ::std::any::TypeId::of::<#p>() },
                        quote! {
                            builder = <#p as ::nestrs_core::Discoverable>::register(builder);
                        },
                    ),
                    ProviderBinding::Dyn { provider, trait_ty } => (
                        provider,
                        path_tail(provider),
                        quote! { ::std::any::TypeId::of::<::std::sync::Arc<#trait_ty>>() },
                        quote! {
                            let __snapshot = builder.snapshot();
                            let __provider = #provider::from_container(&__snapshot);
                            let __dyn: ::std::sync::Arc<#trait_ty> =
                                ::std::sync::Arc::new(__provider);
                            builder = builder.provide_dyn::<#trait_ty>(__dyn);
                        },
                    ),
                };
                let step = quote! {
                    if !__done[#idx] {
                        // Ready when every required dep is present and every
                        // optional dep is present or unsupplied by any pending
                        // provider — keeps order irrelevant.
                        let __required_ready =
                            <#provider as ::nestrs_core::Discoverable>::dependencies()
                                .iter()
                                .all(|__id| builder.contains(*__id));
                        let __optional_ready =
                            <#provider as ::nestrs_core::Discoverable>::optional_dependencies()
                                .iter()
                                .all(|__id| builder.contains(*__id) || !__pending_keys.contains(__id));
                        if __required_ready && __optional_ready {
                            #register_action
                            __done[#idx] = true;
                            __progressed = true;
                        } else {
                            __any_pending = true;
                        }
                    }
                };
                let key_push = quote! {
                    if !__done[#idx] {
                        __pending_keys.push(#provided_key);
                    }
                };
                let classify = quote! {
                    if !__done[#idx] {
                        let __deps = <#provider as ::nestrs_core::Discoverable>::dependencies();
                        let __dep_names =
                            <#provider as ::nestrs_core::Discoverable>::dependency_names();
                        let mut __missing_ids: ::std::vec::Vec<::std::any::TypeId> =
                            ::std::vec::Vec::new();
                        let mut __missing_names: ::std::vec::Vec<&'static str> =
                            ::std::vec::Vec::new();
                        let mut __k = 0usize;
                        while __k < __deps.len() {
                            if !builder.contains(__deps[__k]) {
                                __missing_ids.push(__deps[__k]);
                                __missing_names.push(*__dep_names.get(__k).unwrap_or(&"?"));
                            }
                            __k += 1;
                        }
                        // Pure cycle: every missing dep is one another pending
                        // provider would supply. Otherwise a dep is just absent.
                        if !__missing_ids.is_empty()
                            && __missing_ids.iter().all(|__id| __pending_keys.contains(__id))
                        {
                            __cyclic.push(#name_lit);
                        } else {
                            __unprovided.push(::std::format!(
                                "{} (needs {})", #name_lit, __missing_names.join(", ")
                            ));
                        }
                    }
                };
                (step, key_push, classify)
            })
            .collect();

        let steps = parts.iter().map(|p| &p.0);
        let key_pushes = parts.iter().map(|p| &p.1);
        let classifies = parts.iter().map(|p| &p.2);

        quote! {
            #(#import_calls)*
            let mut __done = [false; #count];
            loop {
                // Provided keys still pending this round — lets an optional dep
                // wait for a same-module provider, and classifies failures.
                let mut __pending_keys: ::std::vec::Vec<::std::any::TypeId> =
                    ::std::vec::Vec::new();
                #(#key_pushes)*
                let mut __any_pending = false;
                let mut __progressed = false;
                #(#steps)*
                if !__any_pending {
                    break;
                }
                if !__progressed {
                    // Stalled: split the two failure modes for an actionable msg.
                    let mut __cyclic: ::std::vec::Vec<&'static str> = ::std::vec::Vec::new();
                    let mut __unprovided: ::std::vec::Vec<::std::string::String> =
                        ::std::vec::Vec::new();
                    #(#classifies)*
                    if !__unprovided.is_empty() {
                        ::std::panic!(
                            "module `{}`: cannot register provider(s) {:?} — each injects a dependency that neither this module's `providers` nor its `imports` registers; add the provider or import the module that exposes it",
                            #name_str, __unprovided
                        );
                    } else {
                        ::std::panic!(
                            "module `{}`: dependency cycle among provider(s) {:?} — each waits on another provider in the same module; break it by injecting `Arc<dyn Trait>` instead of the concrete type",
                            #name_str, __cyclic
                        );
                    }
                }
            }
            builder
        }
    };

    quote! {
        #item

        impl ::nestrs_core::Module for #name {
            fn register(
                mut builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                // Mark before recursing imports so a module cycle terminates.
                if !::nestrs_core::ContainerBuilder::mark_registered(
                    &mut builder,
                    ::std::any::TypeId::of::<#name>(),
                ) {
                    return builder;
                }
                #body
            }

            fn collect(
                mut builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                if !::nestrs_core::ContainerBuilder::mark_collected(
                    &mut builder,
                    ::std::any::TypeId::of::<#name>(),
                ) {
                    return builder;
                }
                #(#collect_calls)*
                builder
            }
        }

        #descriptor_submission
    }
    .into()
}

/// Last path segment for readable boot-time panics.
fn path_tail(p: &Path) -> String {
    p.segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| quote!(#p).to_string())
}

/// Last path segment of a `dyn Trait` for the access-graph descriptor label.
fn path_tail_of_type(ty: &Type) -> String {
    if let Type::TraitObject(obj) = ty {
        for bound in &obj.bounds {
            if let syn::TypeParamBound::Trait(t) = bound {
                if let Some(seg) = t.path.segments.last() {
                    return seg.ident.to_string();
                }
            }
        }
    }
    quote!(#ty).to_string()
}

#[derive(Default)]
struct ModuleArgs {
    imports: Vec<Expr>,
    providers: Vec<ProviderBinding>,
}

/// `MyService` or `MyService as dyn MyTrait` (trait-object binding registered
/// under the trait's `TypeId`).
enum ProviderBinding {
    Concrete(Path),
    Dyn { provider: Path, trait_ty: Box<Type> },
}

impl Parse for ProviderBinding {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let provider: Path = input.parse()?;
        if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            let trait_ty: Type = input.parse()?;
            Ok(Self::Dyn {
                provider,
                trait_ty: Box::new(trait_ty),
            })
        } else {
            Ok(Self::Concrete(provider))
        }
    }
}

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ModuleArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let content;
            bracketed!(content in input);

            match key.to_string().as_str() {
                "imports" => {
                    let exprs: Punctuated<Expr, Token![,]> =
                        Punctuated::parse_terminated(&content)?;
                    args.imports.extend(exprs);
                }
                "providers" => {
                    let bindings: Punctuated<ProviderBinding, Token![,]> =
                        Punctuated::parse_terminated(&content)?;
                    args.providers.extend(bindings);
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown #[module] key `{other}` (expected `imports` or `providers`)"
                        ),
                    ));
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}
