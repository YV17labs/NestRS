use nest_rs_codegen::__private::{
    path_label, residency_match, site, takes_value, type_label, ungrouped_expr,
};
use nest_rs_codegen::Refusals;
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
            Import::Type { ty, .. } => {
                quote! { builder = ::nest_rs_core::ContainerBuilder::import::<#ty>(builder); }
            }
            // The collect phase built the value and parked it at this site, so the
            // expression is evaluated once.
            Import::Value(_) => quote! {
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
            Import::Type { ty, .. } => {
                quote! { builder = ::nest_rs_core::ContainerBuilder::import::<#ty>(builder); }
            }
            Import::Value(value) => quote! {
                builder = ::nest_rs_core::__private::collect_dynamic_import(
                    builder,
                    ::std::any::TypeId::of::<#name>(),
                    #at,
                    #value,
                );
            },
        };
        quote! {
            builder = ::nest_rs_core::__private::enter_import(builder, #name_str, #at, #label);
            #call
            builder = ::nest_rs_core::__private::leave_import(builder);
        }
    });

    // Both phases, after the imports: what a host runs on is there when the
    // fixpoint below builds a provider injecting it.
    let listed = groups.iter().map(|group| match group {
        ProviderGroup::Concrete(p) => p,
        ProviderGroup::Dyn { provider, .. } => provider,
    });
    let host_runtimes = quote! {
        builder = ::nest_rs_core::__private::import_host_runtime(
            builder,
            &[#(::std::any::TypeId::of::<#listed>()),*],
        );
    };

    // A dynamic import is named by its value's type, without evaluating it.
    let import_type_ids = args.imports.iter().map(|import| match import {
        Import::Type { ty, .. } => quote! { || ::std::any::TypeId::of::<#ty>() },
        Import::Value(value) => {
            quote! { || ::nest_rs_core::__private::dynamic_import_module(|| #value) }
        }
    });
    let turbofish_refusals = args.imports.iter().filter_map(turbofish_refusal);
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
                    injects_per_request:
                        <#p as ::nest_rs_core::Discoverable>::injected_per_request,
                    inject_remedies: <#p as ::nest_rs_core::Discoverable>::injected_remedies,
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
                    injects_per_request:
                        <#provider as ::nest_rs_core::Discoverable>::injected_per_request,
                    inject_remedies:
                        <#provider as ::nest_rs_core::Discoverable>::injected_remedies,
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
                let name_lit = path_label(match group {
                    ProviderGroup::Concrete(p) => p,
                    ProviderGroup::Dyn { provider, .. } => provider,
                });
                let (provider, provided_keys, register_action, overridden_action) = match group {
                    ProviderGroup::Concrete(p) => (
                        *p,
                        vec![quote! { ::std::any::TypeId::of::<#p>() }],
                        quote! {
                            builder = <#p as ::nest_rs_core::Discoverable>::register(builder);
                        },
                        quote! {
                            builder = <#p as ::nest_rs_core::Discoverable>::__nestrs_register_overridden(
                                builder,
                            );
                        },
                    ),
                    ProviderGroup::Dyn { provider, traits } => {
                        let keys: Vec<proc_macro2::TokenStream> =
                            std::iter::once(quote! { ::std::any::TypeId::of::<#provider>() })
                                .chain(traits.iter().map(|trait_ty| {
                                    quote! {
                                        ::std::any::TypeId::of::<::std::sync::Arc<#trait_ty>>()
                                    }
                                }))
                                .collect();
                        let register = quote! {
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
                        };
                        let named_keys = std::iter::once(quote! { #provider })
                            .chain(traits.iter().map(|trait_ty| {
                                quote! { ::std::sync::Arc<#trait_ty> }
                            }))
                            .map(|key| {
                                quote! {
                                    (
                                        ::std::any::TypeId::of::<#key>(),
                                        ::std::any::type_name::<#key>(),
                                    )
                                }
                            });
                        // One key overridden stands in for the whole binding.
                        let overridden = quote! {
                            builder = ::nest_rs_core::__private::claim_override(
                                builder,
                                ::std::any::TypeId::of::<#provider>(),
                                #name_lit,
                                ::nest_rs_core::ProviderScope::Singleton,
                                &[ #(#named_keys),* ],
                            );
                        };
                        (*provider, keys, register, overridden)
                    }
                };
                let step = quote! {
                    if !__done[#idx]
                        && ::nest_rs_core::__private::overrides_any(&builder, &[ #(#provided_keys),* ])
                    {
                        // Never built: its dependencies need not exist.
                        #overridden_action
                        __done[#idx] = true;
                        __progressed = true;
                    }
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
            #host_runtimes
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
    // the developer wrote, since a const panic takes a literal naming no type.
    let residency_checks = groups.iter().filter_map(|group| match group {
        ProviderGroup::Dyn { provider, traits } => {
            let name = path_label(provider);
            let bound = traits
                .iter()
                .map(|t| format!("as {}", type_label(t)))
                .collect::<Vec<_>>()
                .join(" and ");
            let refusal = |cause: &str, remedy: &str| {
                format!(
                    "`{name}` is bound `{bound}`, which holds one instance for the app's \
                     lifetime, and `{name}` {cause}: bind a plain `#[injectable]` singleton{remedy}"
                )
            };
            Some(residency_match(
                provider.span(),
                provider,
                [
                    refusal("is an edge host, built at mount", ""),
                    refusal(
                        "is `scope = request`, built per request",
                        &format!(", and reach `{name}` through a `Scoped<T>` parameter"),
                    ),
                    refusal("is `scope = transient`, rebuilt at every resolution", ""),
                    refusal("states `ProviderResidency::SINGLETON = false`", ""),
                ],
            ))
        }
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
                #host_runtimes
                builder
            }
        }

        #descriptor_submission

        #(#turbofish_refusals)*
    }
    .into()
}

/// An import as a boot error names it — `HttpModule::for_root(..)`, the
/// arguments left out, since a pinned config may hold a secret.
fn import_label(import: &Import) -> String {
    let spelled = |tokens: proc_macro2::TokenStream| {
        tokens.to_string().split_whitespace().collect::<String>()
    };
    match import {
        Import::Type { ty, .. } => spelled(quote!(#ty)),
        Import::Value(Expr::Call(call)) => match &*call.func {
            Expr::Path(func) => format!("{}(..)", spelled(quote!(#func))),
            _ => "an import expression".to_owned(),
        },
        Import::Value(_) => "an import expression".to_owned(),
    }
}

/// The refusal of a module written with the turbofish the published release taught, spanned at
/// the developer's `::`: the module is still read as its type, so this is the
/// one error.
fn turbofish_refusal(import: &Import) -> Option<proc_macro2::TokenStream> {
    let Import::Type {
        turbofish: Some(at),
        ..
    } = import
    else {
        return None;
    };
    let refusal = format!(
        "{}: write `{}`, a module as its type, without `::` before `<`",
        site("module", Some("imports")),
        import_label(import),
    );
    Some(quote_spanned! {*at=> ::core::compile_error!(#refusal); })
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
    imports: Vec<Import>,
    providers: Vec<ProviderBinding>,
}

const IMPORTS_TAKE: &str = "a list of modules, e.g. `imports = [UsersModule, \
                            RedisModule<Cache>, HttpModule::for_root(cfg)]`";

/// One entry of `imports`, as written.
enum Import {
    /// A module imported by its type, `UsersModule` or `RedisModule<Cache>`,
    /// with where a turbofish stood when it was written `RedisModule::<Cache>`.
    Type {
        ty: syn::TypePath,
        turbofish: Option<proc_macro2::Span>,
    },
    /// A configured value, evaluated once at collect: `HttpModule::for_root(cfg)`.
    Value(Expr),
}

impl Import {
    /// `ty` as the type it names, any turbofish taken out of it.
    fn of_type(mut ty: syn::TypePath) -> Self {
        let mut turbofish = None;
        for segment in &mut ty.path.segments {
            if let syn::PathArguments::AngleBracketed(arguments) = &mut segment.arguments
                && let Some(colon2) = arguments.colon2_token.take()
            {
                turbofish.get_or_insert(colon2.spans[0]);
            }
        }
        Self::Type { ty, turbofish }
    }
}

impl Parse for Import {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // An expression first, as `for_root(..)` is one: `RedisModule<Cache>` is no
        // expression, since an expression reads its `<` as a comparison.
        let expression = input.fork();
        if expression.parse::<Expr>().is_ok()
            && (expression.is_empty() || expression.peek(Token![,]))
        {
            let value: Expr = input.parse()?;
            return Ok(match ungrouped_expr(&value) {
                Expr::Path(path) => Self::of_type(syn::TypePath {
                    attrs: path.attrs.clone(),
                    qself: path.qself.clone(),
                    path: path.path.clone(),
                }),
                _ => Self::Value(value),
            });
        }
        input.parse().map(Self::of_type)
    }
}

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
    let site = site("module", Some("providers"));
    let mut groups: Vec<(String, ProviderGroup<'_>)> = Vec::new();
    let mut refusals = Refusals::default();
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
        refusals.push(syn::Error::new_spanned(binding, message));
    }
    refusals.finish()?;
    Ok(groups.into_iter().map(|(_, group)| group).collect())
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
                    takes_value("module", Some(name), takes),
                ));
            }
            let content;
            bracketed!(content in input);
            if name == "imports" {
                let imports: Punctuated<Import, Token![,]> = Punctuated::parse_terminated(&content)
                    .map_err(|stopped| {
                        syn::Error::new(stopped.span(), takes_value("module", Some(name), takes))
                    })?;
                args.imports.extend(imports);
            } else {
                let bindings: Punctuated<ProviderBinding, Token![,]> =
                    Punctuated::parse_terminated(&content).map_err(|stopped| {
                        syn::Error::new(stopped.span(), takes_value("module", Some(name), takes))
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

    fn parsed<T: Parse>(tokens: proc_macro2::TokenStream) -> syn::Result<Vec<T>> {
        let parse = Punctuated::<T, Token![,]>::parse_terminated;
        syn::parse::Parser::parse2(parse, tokens).map(|read| read.into_iter().collect())
    }

    fn list(tokens: proc_macro2::TokenStream) -> Vec<ProviderBinding> {
        parsed(tokens).expect("a providers list")
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

    fn imports(tokens: proc_macro2::TokenStream) -> Vec<Import> {
        parsed(tokens).expect("every entry is an import")
    }

    /// A path a `macro_rules!` forwards as an `$m:expr`, in its invisible group.
    fn grouped(tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        let group = proc_macro2::Group::new(proc_macro2::Delimiter::None, tokens);
        proc_macro2::TokenTree::Group(group).into()
    }

    #[test]
    fn an_import_is_a_module_type_however_its_generics_are_written_or_a_value() {
        let forwarded = grouped(quote! { RedisModule::<Cache> });
        let read = imports(quote! {
            UsersModule,
            RedisModule<Cache>,
            crate::cache::RedisModule<crate::Cache>,
            RedisModule::<Cache>,
            #forwarded,
            HttpModule::for_root(cfg),
            RedisModule::for_root((Cache, cfg)),
        });
        let kinds: Vec<String> = read
            .iter()
            .map(|import| {
                let kind = match import {
                    Import::Type { .. } => "type",
                    Import::Value(_) => "value",
                };
                format!("{kind} {}", import_label(import))
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "type UsersModule",
                "type RedisModule<Cache>",
                "type crate::cache::RedisModule<crate::Cache>",
                "type RedisModule<Cache>",
                "type RedisModule<Cache>",
                "value HttpModule::for_root(..)",
                "value RedisModule::for_root(..)",
            ]
        );
    }

    #[test]
    fn only_a_turbofish_is_refused_and_its_refusal_spells_the_module_as_its_type() {
        let forwarded = grouped(quote! { RedisModule::<Cache> });
        let refusals: Vec<Option<String>> = imports(quote! {
            UsersModule, RedisModule<Cache>, RedisModule::<Cache>, #forwarded,
            HttpModule::for_root(cfg)
        })
        .iter()
        .map(|import| turbofish_refusal(import).map(|refusal| refusal.to_string()))
        .collect();
        let refused: Vec<bool> = refusals.iter().map(Option::is_some).collect();
        assert_eq!(refused, [false, false, true, true, false]);
        let refusal = refusals[2].as_deref().unwrap_or_default();
        assert!(
            refusal.contains("#[module] `imports`: write `RedisModule<Cache>`"),
            "{refusal}"
        );
    }

    #[test]
    fn an_entry_neither_a_module_type_nor_a_value_is_refused_naming_what_imports_take() {
        for tokens in [
            quote! { imports = [UsersModule UsersModule] },
            quote! { imports = [RedisModule<Cache> extra] },
        ] {
            let refused = syn::parse2::<ModuleArgs>(tokens)
                .err()
                .expect("no import reads this way")
                .to_string();
            assert!(
                refused.starts_with("#[module] `imports` takes "),
                "{refused}"
            );
            assert!(refused.contains("RedisModule<Cache>"), "{refused}");
        }
    }
}
