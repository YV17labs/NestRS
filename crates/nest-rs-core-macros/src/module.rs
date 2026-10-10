use nest_rs_codegen::last_segment_ident;
use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, ItemStruct, Path, Token, Type, bracketed, parse_macro_input};

pub(crate) fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ModuleArgs);
    let item = parse_macro_input!(input as ItemStruct);
    let name = item.ident.clone();
    let name_str = name.to_string();

    // `enter_import`/`leave_import` let a contested declaration's boot error
    // name the module and position that imported it.
    let import_calls = args.imports.iter().enumerate().map(|(i, import)| {
        let at = proc_macro2::Literal::usize_unsuffixed(i);
        let label = import_label(import);
        let call = match import {
            Expr::Path(p) => {
                let path = &p.path;
                quote! { builder = ::nest_rs_core::ContainerBuilder::import::<#path>(builder); }
            }
            // The collect phase built the value and parked it at this site, so the
            // expression is evaluated once.
            _ => quote! {
                builder = ::nest_rs_core::__private::register_dynamic_import(
                    builder,
                    ::std::any::TypeId::of::<#name>(),
                    #at,
                );
            },
        };
        quote! {
            builder = ::nest_rs_core::__private::enter_import(builder, #name_str, #at, #label);
            #call
            builder = ::nest_rs_core::__private::leave_import(builder);
        }
    });

    let collect_calls = args.imports.iter().enumerate().map(|(i, import)| {
        let at = proc_macro2::Literal::usize_unsuffixed(i);
        let label = import_label(import);
        let call = match import {
            Expr::Path(p) => {
                let path = &p.path;
                quote! { builder = ::nest_rs_core::ContainerBuilder::import::<#path>(builder); }
            }
            other => quote! {
                builder = ::nest_rs_core::__private::collect_dynamic_import(
                    builder,
                    ::std::any::TypeId::of::<#name>(),
                    #at,
                    #other,
                );
            },
        };
        quote! {
            builder = ::nest_rs_core::__private::enter_import(builder, #name_str, #at, #label);
            #call
            builder = ::nest_rs_core::__private::leave_import(builder);
        }
    });

    // A dynamic import is named by its value's type, without evaluating it.
    let import_type_ids = args.imports.iter().map(|import| match import {
        Expr::Path(p) => {
            let path = &p.path;
            quote! { || ::std::any::TypeId::of::<#path>() }
        }
        other => quote! { || ::nest_rs_core::__private::dynamic_import_module(|| #other) },
    });
    let provider_descriptors = args.providers.iter().map(|binding| match binding {
        ProviderBinding::Concrete(p) => {
            let name_lit = path_tail(p);
            quote! {
                ::nest_rs_core::__private::ProviderDescriptor {
                    name: #name_lit,
                    provides: || ::std::any::TypeId::of::<#p>(),
                    provider: || ::std::any::TypeId::of::<#p>(),
                    also_provides: <#p as ::nest_rs_core::Discoverable>::also_provides,
                    injects: <#p as ::nest_rs_core::Discoverable>::injected,
                    inject_names: <#p as ::nest_rs_core::Discoverable>::injected_names,
                    injects_optional: <#p as ::nest_rs_core::Discoverable>::injected_optional,
                    injects_keyed: <#p as ::nest_rs_core::Discoverable>::injected_keyed,
                }
            }
        }
        ProviderBinding::Dyn { provider, trait_ty } => {
            let name_lit = format!("dyn {}", path_tail_of_type(trait_ty));
            quote! {
                ::nest_rs_core::__private::ProviderDescriptor {
                    name: #name_lit,
                    provides: || ::std::any::TypeId::of::<::std::sync::Arc<#trait_ty>>(),
                    provider: || ::std::any::TypeId::of::<#provider>(),
                    also_provides: <#provider as ::nest_rs_core::Discoverable>::also_provides,
                    injects: <#provider as ::nest_rs_core::Discoverable>::injected,
                    inject_names: <#provider as ::nest_rs_core::Discoverable>::injected_names,
                    injects_optional: <#provider as ::nest_rs_core::Discoverable>::injected_optional,
                    injects_keyed: <#provider as ::nest_rs_core::Discoverable>::injected_keyed,
                }
            }
        }
    });
    let descriptor_submission = quote! {
        ::nest_rs_core::inventory::submit! {
            ::nest_rs_core::__private::ModuleDescriptor {
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
            ::nest_rs_core::__private::module_registered(#name_str);
            builder
        }
    } else {
        let count = proc_macro2::Literal::usize_unsuffixed(args.providers.len());
        // Per provider: its register attempt, its provided key, and a stall-time
        // classification of why it is still pending.
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
                            builder = <#p as ::nest_rs_core::Discoverable>::register(builder);
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
                        // An optional dep waits only while a pending provider supplies it.
                        let __required_ready =
                            <#provider as ::nest_rs_core::Discoverable>::dependencies()
                                .iter()
                                .all(|__id| builder.contains(*__id));
                        let __optional_ready =
                            <#provider as ::nest_rs_core::Discoverable>::optional_dependencies()
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
                        let __deps = <#provider as ::nest_rs_core::Discoverable>::dependencies();
                        let __dep_names =
                            <#provider as ::nest_rs_core::Discoverable>::dependency_names();
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
                        // A cycle when every missing dep is one a pending provider supplies.
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
                    // A missing dependency is left to the access-graph check; a
                    // cycle is invisible to it, so it is refused here.
                    let mut __cyclic: ::std::vec::Vec<&'static str> = ::std::vec::Vec::new();
                    let mut __unprovided: ::std::vec::Vec<::std::string::String> =
                        ::std::vec::Vec::new();
                    #(#classifies)*
                    if __unprovided.is_empty() {
                        builder = builder.refuse(::nest_rs_core::ProviderCycleError {
                            module: #name_str,
                            type_names: __cyclic,
                        });
                    }
                    break;
                }
            }
            ::nest_rs_core::__private::module_registered(#name_str);
            builder
        }
    };

    quote! {
        #item

        impl ::nest_rs_core::Module for #name {
            fn register(
                mut builder: ::nest_rs_core::ContainerBuilder,
                _: ::nest_rs_core::Registering<Self>,
            ) -> ::nest_rs_core::ContainerBuilder {
                #body
            }

            fn collect(
                mut builder: ::nest_rs_core::ContainerBuilder,
                _: ::nest_rs_core::Collecting<Self>,
            ) -> ::nest_rs_core::ContainerBuilder {
                #(#collect_calls)*
                builder
            }
        }

        #descriptor_submission
    }
    .into()
}

/// An import as a boot error names it — `HttpModule::for_root(..)`, the
/// arguments left out, since a pinned config may hold a secret.
fn import_label(import: &Expr) -> String {
    let spelled = |tokens: proc_macro2::TokenStream| {
        tokens.to_string().split_whitespace().collect::<String>()
    };
    match import {
        Expr::Path(p) => spelled(quote!(#p)),
        Expr::Call(call) => match &*call.func {
            Expr::Path(func) => format!("{}(..)", spelled(quote!(#func))),
            _ => "an import expression".to_owned(),
        },
        _ => "an import expression".to_owned(),
    }
}

fn path_tail(p: &Path) -> String {
    last_segment_ident(p).to_string()
}

fn path_tail_of_type(ty: &Type) -> String {
    if let Type::TraitObject(obj) = ty {
        for bound in &obj.bounds {
            if let syn::TypeParamBound::Trait(t) = bound
                && let Some(seg) = t.path.segments.last()
            {
                return seg.ident.to_string();
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

const IMPORTS_TAKE: &str =
    "a list of modules, e.g. `imports = [UsersModule, HttpModule::for_root(None)]`";

const PROVIDERS_TAKE: &str = "a list of provider types, e.g. `providers = [UsersService]`, or \
     `Store as dyn Cache` to register one under a trait object";

/// `MyService` or `MyService as dyn MyTrait`.
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

const MODULE: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("module", &["imports", "providers"]);

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ModuleArgs::default();
        MODULE.parse(input, |arg| {
            let name = arg.key();
            let input = arg.value()?;
            let takes = if name == "imports" {
                IMPORTS_TAKE
            } else {
                PROVIDERS_TAKE
            };
            if !input.peek(syn::token::Bracket) {
                return Err(syn::Error::new(
                    input.span(),
                    nest_rs_codegen::takes_value("module", Some(name), takes),
                ));
            }
            let content;
            bracketed!(content in input);
            if name == "imports" {
                let exprs: Punctuated<Expr, Token![,]> = Punctuated::parse_terminated(&content)?;
                args.imports.extend(exprs);
            } else {
                let bindings: Punctuated<ProviderBinding, Token![,]> =
                    Punctuated::parse_terminated(&content).map_err(|stopped| {
                        syn::Error::new(
                            stopped.span(),
                            nest_rs_codegen::takes_value("module", Some(name), takes),
                        )
                    })?;
                args.providers.extend(bindings);
            }
            Ok(())
        })?;
        Ok(args)
    }
}
