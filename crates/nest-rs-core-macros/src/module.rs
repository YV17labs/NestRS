use nest_rs_codegen::path_label;
use proc_macro::TokenStream;
use quote::{ToTokens, quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, ItemStruct, Path, Token, Type, bracketed, parse_macro_input};

pub(crate) fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ModuleArgs);
    let item = parse_macro_input!(input as ItemStruct);
    if !item.generics.params.is_empty() {
        let refusal = syn::Error::new_spanned(
            &item.generics,
            "a #[module] is not generic: it submits one static descriptor for the access graph. \
             A module generic over an `Instance` is written by hand — \
             `impl<I: Instance> Module for RedisQueueModule<I>` — as adapter bindings are",
        )
        .to_compile_error();
        return quote! { #item #refusal }.into();
    }
    let name = item.ident.clone();
    let name_str = name.to_string();
    let groups = match group_providers(&args.providers) {
        Ok(groups) => groups,
        Err(err) => return err.to_compile_error().into(),
    };

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
            let name_lit = path_label(p);
            quote! {
                ::nest_rs_core::__private::ProviderDescriptor {
                    name: #name_lit,
                    provides: || ::std::any::TypeId::of::<#p>(),
                    provider: || ::std::any::TypeId::of::<#p>(),
                    also_provides: <#p as ::nest_rs_core::Discoverable>::also_provides,
                    injects: <#p as ::nest_rs_core::Discoverable>::injected,
                    inject_names: <#p as ::nest_rs_core::Discoverable>::injected_names,
                    injects_optional: <#p as ::nest_rs_core::Discoverable>::injected_optional,
                    inject_optional_names:
                        <#p as ::nest_rs_core::Discoverable>::injected_optional_names,
                    injects_keyed: <#p as ::nest_rs_core::Discoverable>::injected_keyed,
                }
            }
        }
        ProviderBinding::Dyn { provider, trait_ty } => {
            let name_lit = format!("dyn {}", path_tail_of_type(trait_ty));
            let provider_lit = path_label(provider);
            quote! {
                ::nest_rs_core::__private::ProviderDescriptor {
                    name: #name_lit,
                    provides: || ::std::any::TypeId::of::<::std::sync::Arc<#trait_ty>>(),
                    provider: || ::std::any::TypeId::of::<#provider>(),
                    also_provides: || {
                        let mut __keys =
                            <#provider as ::nest_rs_core::Discoverable>::also_provides();
                        __keys.push((::std::any::TypeId::of::<#provider>(), #provider_lit));
                        __keys
                    },
                    injects: <#provider as ::nest_rs_core::Discoverable>::injected,
                    inject_names: <#provider as ::nest_rs_core::Discoverable>::injected_names,
                    injects_optional: <#provider as ::nest_rs_core::Discoverable>::injected_optional,
                    inject_optional_names:
                        <#provider as ::nest_rs_core::Discoverable>::injected_optional_names,
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

    let body = if groups.is_empty() {
        quote! {
            #(#import_calls)*
            ::nest_rs_core::__private::module_registered(#name_str);
            builder
        }
    } else {
        let count = proc_macro2::Literal::usize_unsuffixed(groups.len());
        // Per provider: its register attempt, the keys it provides, and what
        // the stall reports of it.
        let parts: Vec<(
            proc_macro2::TokenStream,
            proc_macro2::TokenStream,
            proc_macro2::TokenStream,
        )> = groups
            .iter()
            .enumerate()
            .map(|(i, group)| {
                let idx = proc_macro2::Literal::usize_unsuffixed(i);
                let (provider, provided_keys, register_action) = match group {
                    ProviderGroup::Concrete(p) => (
                        *p,
                        vec![quote! { ::std::any::TypeId::of::<#p>() }],
                        quote! {
                            builder = <#p as ::nest_rs_core::Discoverable>::register(builder);
                        },
                    ),
                    ProviderGroup::Dyn { provider, traits } => (
                        *provider,
                        std::iter::once(quote! { ::std::any::TypeId::of::<#provider>() })
                            .chain(traits.iter().map(|trait_ty| {
                                quote! {
                                    ::std::any::TypeId::of::<::std::sync::Arc<#trait_ty>>()
                                }
                            }))
                            .collect(),
                        quote! {
                            let __provider: ::std::sync::Arc<#provider> = ::std::sync::Arc::new(
                                <#provider>::__nestrs_from_container(&builder.snapshot()),
                            );
                            builder = builder
                                .provide_arc::<#provider>(::std::sync::Arc::clone(&__provider));
                            #(
                                builder = builder.provide_dyn::<#traits>(
                                    ::std::sync::Arc::clone(&__provider)
                                        as ::std::sync::Arc<#traits>,
                                );
                            )*
                        },
                    ),
                };
                let name_lit = path_label(provider);
                let step = quote! {
                    if !__done[#idx] {
                        // An optional dep waits only while a pending provider supplies it.
                        let __required_ready =
                            <#provider as ::nest_rs_core::Discoverable>::dependencies()
                                .iter()
                                .all(|__id| builder.contains(*__id));
                        let __optional =
                            <#provider as ::nest_rs_core::Discoverable>::optional_dependencies();
                        let __optional_ready = __optional
                            .iter()
                            .all(|__id| builder.contains(*__id) || !__pending_keys.contains(__id));
                        if __required_ready && __optional_ready {
                            builder = ::nest_rs_core::__private::record_absent_optional(
                                builder,
                                ::std::any::TypeId::of::<#name>(),
                                #name_str,
                                ::std::any::TypeId::of::<#provider>(),
                                #name_lit,
                                &__optional,
                                &<#provider as ::nest_rs_core::Discoverable>::optional_dependency_names(),
                            );
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
                        #( __pending_keys.push(#provided_keys); )*
                    }
                };
                let stall = quote! {
                    if !__done[#idx] {
                        __stalled.push(::nest_rs_core::__private::StalledProvider {
                            name: #name_lit,
                            provides: ::std::vec![ #(#provided_keys),* ],
                            dependencies:
                                <#provider as ::nest_rs_core::Discoverable>::dependencies(),
                            dependency_names:
                                <#provider as ::nest_rs_core::Discoverable>::dependency_names(),
                            optional_dependencies:
                                <#provider as ::nest_rs_core::Discoverable>::optional_dependencies(),
                            optional_dependency_names:
                                <#provider as ::nest_rs_core::Discoverable>::optional_dependency_names(),
                        });
                    }
                };
                (step, key_push, stall)
            })
            .collect();

        let steps = parts.iter().map(|p| &p.0);
        let key_pushes = parts.iter().map(|p| &p.1);
        let stalls = parts.iter().map(|p| &p.2);

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
                    let mut __stalled: ::std::vec::Vec<::nest_rs_core::__private::StalledProvider> =
                        ::std::vec::Vec::new();
                    #(#stalls)*
                    builder = ::nest_rs_core::__private::record_stall(
                        builder,
                        ::std::any::TypeId::of::<#name>(),
                        #name_str,
                        __stalled,
                    );
                    break;
                }
            }
            ::nest_rs_core::__private::module_registered(#name_str);
            builder
        }
    };

    // A const item, so `cargo check` evaluates it; spanned on the provider path
    // the developer wrote, since `assert!` takes a literal naming no type.
    let residency_checks = groups.iter().filter_map(|group| match group {
        ProviderGroup::Dyn { provider, .. } => Some(quote_spanned! {provider.span()=>
            const _: () = ::core::assert!(
                <#provider as ::nest_rs_core::ProviderResidency>::SINGLETON,
                "`as dyn Trait` binds one instance built at boot for the app's lifetime, so the \
                 provider must be a singleton the container holds. This one is not: an edge host \
                 (#[controller], #[gateway], #[resolver], #[mcp]) is built at mount, \
                 `scope = request` builds one per request, and `scope = transient` one per \
                 resolution. Bind a plain #[injectable] singleton; reach a scoped provider \
                 through the edge's `Scoped<T>`.",
            );
        }),
        ProviderGroup::Concrete(_) => None,
    });

    quote! {
        #item

        #(#residency_checks)*

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

/// One provider type of a `providers` list, as written: listed bare, or bound
/// under each trait it is listed `as dyn` with — never both, never twice.
enum ProviderGroup<'a> {
    Concrete(&'a Path),
    Dyn {
        provider: &'a Path,
        traits: Vec<&'a Type>,
    },
}

/// The list grouped by provider path as written, refusing at the second entry a
/// provider listed twice or listed bare beside its `as dyn` binding.
fn group_providers(bindings: &[ProviderBinding]) -> syn::Result<Vec<ProviderGroup<'_>>> {
    let site = nest_rs_codegen::site("module", Some("providers"));
    let mut groups: Vec<(String, ProviderGroup<'_>)> = Vec::new();
    let mut refusal: Option<syn::Error> = None;
    for binding in bindings {
        let (provider, trait_ty) = match binding {
            ProviderBinding::Concrete(p) => (p, None),
            ProviderBinding::Dyn { provider, trait_ty } => (provider, Some(&**trait_ty)),
        };
        let spelled = written(provider);
        let Some((_, group)) = groups.iter_mut().find(|(seen, _)| *seen == spelled) else {
            let group = match trait_ty {
                None => ProviderGroup::Concrete(provider),
                Some(trait_ty) => ProviderGroup::Dyn {
                    provider,
                    traits: vec![trait_ty],
                },
            };
            groups.push((spelled, group));
            continue;
        };
        let message = match (group, trait_ty) {
            (ProviderGroup::Dyn { traits, .. }, Some(trait_ty))
                if !traits.iter().any(|seen| written(seen) == written(trait_ty)) =>
            {
                traits.push(trait_ty);
                continue;
            }
            (ProviderGroup::Concrete(_), None) | (ProviderGroup::Dyn { .. }, Some(_)) => format!(
                "{site}: `{}` is listed twice — a provider is listed once",
                written(binding)
            ),
            (ProviderGroup::Concrete(_), Some(trait_ty)) => beside(&site, &spelled, trait_ty),
            (ProviderGroup::Dyn { traits, .. }, None) => beside(&site, &spelled, traits[0]),
        };
        let error = syn::Error::new_spanned(binding, message);
        match &mut refusal {
            Some(first) => first.combine(error),
            None => refusal = Some(error),
        }
    }
    match refusal {
        Some(error) => Err(error),
        None => Ok(groups.into_iter().map(|(_, group)| group).collect()),
    }
}

/// The refusal of a bare `provider` beside its binding `as trait_ty`.
fn beside(site: &str, provider: &str, trait_ty: &Type) -> String {
    format!(
        "{site}: `{provider}` is listed beside `{provider} as {}`, which already holds it under \
         its own type — remove the bare entry",
        written(trait_ty)
    )
}

/// Tokens as a developer wrote them: `crate::Store`, `Store as dyn Cache`.
fn written(tokens: &impl ToTokens) -> String {
    tokens
        .to_token_stream()
        .to_string()
        .replace(" :: ", "::")
        .replace(":: ", "::")
        .replace(" < ", "<")
        .replace(" >", ">")
}

/// `MyService` or `MyService as dyn MyTrait`.
enum ProviderBinding {
    Concrete(Path),
    Dyn { provider: Path, trait_ty: Box<Type> },
}

/// The entry as written, for a refusal's words and span.
impl ToTokens for ProviderBinding {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            Self::Concrete(p) => p.to_tokens(tokens),
            Self::Dyn { provider, trait_ty } => tokens.extend(quote!(#provider as #trait_ty)),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn list(tokens: proc_macro2::TokenStream) -> Vec<ProviderBinding> {
        let parse = Punctuated::<ProviderBinding, Token![,]>::parse_terminated;
        syn::parse::Parser::parse2(parse, tokens)
            .expect("a providers list")
            .into_iter()
            .collect()
    }

    #[test]
    fn one_provider_bound_as_two_traits_is_one_group_in_first_entry_order() {
        let bindings = list(quote! { Store as dyn Cache, Users, Store as dyn Flush });
        let groups = group_providers(&bindings).expect("two traits are no repeat");
        assert_eq!(groups.len(), 2);
        let ProviderGroup::Dyn { traits, .. } = &groups[0] else {
            panic!("the binding keeps its first entry's slot");
        };
        assert_eq!(traits.len(), 2);
        assert!(matches!(groups[1], ProviderGroup::Concrete(_)));
    }

    #[test]
    fn a_repeat_is_refused_at_its_site_in_the_words_written() {
        for (tokens, says) in [
            (
                quote! { crate::Store, crate::Store },
                "`crate::Store` is listed twice — a provider is listed once",
            ),
            (
                quote! { Store as dyn Cache<u8>, Store as dyn Cache<u8> },
                "`Store as dyn Cache<u8>` is listed twice",
            ),
            (
                quote! { Store as dyn Cache, Store },
                "`Store` is listed beside `Store as dyn Cache`, which already holds it under \
                 its own type — remove the bare entry",
            ),
            (
                quote! { Store, Store as dyn Cache },
                "`Store` is listed beside `Store as dyn Cache`",
            ),
        ] {
            let refused = group_providers(&list(tokens))
                .err()
                .expect("a repeat is refused")
                .to_string();
            assert!(refused.starts_with("#[module] `providers`: "), "{refused}");
            assert!(refused.contains(says), "{refused}");
        }
    }
}
