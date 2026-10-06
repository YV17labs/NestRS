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

    // Every import's phase runs between `enter_import` and `leave_import`, so a
    // declaration it makes is named by the module and the position that
    // imported it — what a contested declaration's boot error points at.
    let import_calls = args.imports.iter().enumerate().map(|(i, import)| {
        let at = proc_macro2::Literal::usize_unsuffixed(i);
        let label = import_label(import);
        let call = match import {
            // Bare type path → static `Module`.
            Expr::Path(p) => {
                let path = &p.path;
                quote! { builder = <#path as ::nest_rs_core::Module>::register(builder); }
            }
            // Anything else → `DynamicModule` value (e.g. `Module::for_root(opts)`).
            // The collect phase built the value and parked it at this site, so
            // register consumes *that* value: the expression is written, and
            // evaluated, once (CORE-I9).
            _ => quote! {
                builder = ::nest_rs_core::ContainerBuilder::register_dynamic_import(
                    builder,
                    ::std::any::TypeId::of::<#name>(),
                    #at,
                );
            },
        };
        quote! {
            builder = builder.enter_import(#name_str, #at, #label);
            #call
            builder = builder.leave_import();
        }
    });

    // Collect phase only queues async factories; providers untouched here. The
    // dynamic import is constructed here and parked for the register phase.
    let collect_calls = args.imports.iter().enumerate().map(|(i, import)| {
        let at = proc_macro2::Literal::usize_unsuffixed(i);
        let label = import_label(import);
        let call = match import {
            Expr::Path(p) => {
                let path = &p.path;
                quote! { builder = <#path as ::nest_rs_core::Module>::collect(builder); }
            }
            other => quote! {
                builder = ::nest_rs_core::ContainerBuilder::collect_dynamic_import(
                    builder,
                    ::std::any::TypeId::of::<#name>(),
                    #at,
                    #other,
                );
            },
        };
        quote! {
            builder = builder.enter_import(#name_str, #at, #label);
            #call
            builder = builder.leave_import();
        }
    });

    // Access-graph descriptor submitted to the link-time registry. A dynamic
    // import is named by the module its value's type declares, read off the
    // expression without evaluating it.
    let import_type_ids = args.imports.iter().map(|import| match import {
        Expr::Path(p) => {
            let path = &p.path;
            quote! { || ::std::any::TypeId::of::<#path>() }
        }
        other => quote! { || ::nest_rs_core::__dynamic_import_module(|| #other) },
    });
    let provider_descriptors = args.providers.iter().map(|binding| match binding {
        ProviderBinding::Concrete(p) => {
            let name_lit = path_tail(p);
            quote! {
                ::nest_rs_core::ProviderDescriptor {
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
                ::nest_rs_core::ProviderDescriptor {
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
            ::nest_rs_core::ModuleDescriptor {
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
            ::nest_rs_core::__module_registered(#name_str);
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
                        // Ready when every required dep is present and every
                        // optional dep is present or unsupplied by any pending
                        // provider — keeps order irrelevant.
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
                    // Stalled: split the two failure modes. A genuinely-missing
                    // dependency is *deferred* to the boot-time access-graph check
                    // (`App::new` / `App::builder().build()`), which fails with a
                    // named `MissingDependencyError` / `AccessGraphError`. A true
                    // cycle (no missing dep, providers only waiting on each other)
                    // is invisible to the graph, so it is refused here — either
                    // way every wiring failure surfaces through the same `Result`.
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
                    // Leave the unbuilt providers out; the access-graph check
                    // names a missing dependency and fails the boot cleanly.
                    break;
                }
            }
            ::nest_rs_core::__module_registered(#name_str);
            builder
        }
    };

    quote! {
        #item

        impl ::nest_rs_core::Module for #name {
            fn register(
                mut builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                // Mark before recursing imports so a module cycle terminates.
                if !::nest_rs_core::ContainerBuilder::mark_registered(
                    &mut builder,
                    ::std::any::TypeId::of::<#name>(),
                ) {
                    return builder;
                }
                // A no-op once collected. A setup that left this module's
                // `collect` out gets it here, too late for a factory, which the
                // boot then refuses by name rather than leaving it unqueued.
                builder = <#name as ::nest_rs_core::Module>::collect(builder);
                #body
            }

            fn collect(
                mut builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                if !::nest_rs_core::ContainerBuilder::mark_collected(
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

/// An import as a boot error names its site: a module by its path, a dynamic
/// import by the function that builds it — `HttpModule::for_root(..)`, the
/// arguments left out, since a pinned config is no part of where it was
/// declared and may hold a secret.
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

/// Last path segment for readable boot-time panics.
fn path_tail(p: &Path) -> String {
    last_segment_ident(p).to_string()
}

/// Last path segment of a `dyn Trait` for the access-graph descriptor label.
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

/// What `imports` takes, in the sentence a value of another kind is refused with.
const IMPORTS_TAKE: &str =
    "a list of modules, e.g. `imports = [UsersModule, HttpModule::for_root(None)]`";

/// What `providers` takes — including the one spelling beside a bare type.
const PROVIDERS_TAKE: &str = "a list of provider types, e.g. `providers = [UsersService]`, or \
     `Store as dyn Cache` to register one under a trait object";

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

/// `#[module]`'s two keys.
const MODULE: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("module", &["imports", "providers"]);

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ModuleArgs::default();
        // Each key is judged **before** its value is read — by the grammar, which
        // hands over only a known key written for the first time. Reading `= [`
        // first meant the unknown-key sentence was reachable only when the wrong
        // key happened to take a bracketed value: `#[module(exports = [Foo])]`
        // named the key, `#[module(porviders = Foo)]` answered `expected square
        // brackets`.
        //
        // A repeat is refused rather than merged. It is legible here — two
        // `providers = [...]` lists concatenate — so nothing is *dropped*, which
        // is why `duplicate_argument`'s own reasoning does not apply verbatim.
        // What applies is that every other member of the `key = value` family
        // refuses it, and a grammar the framework interprets accepting a
        // spelling its siblings reject is the asymmetry a shared sentence exists
        // to remove: one list is what a reader can see whole.
        MODULE.parse(input, |arg| {
            let name = arg.key();
            let input = arg.value()?;
            // Both keys take a list, and a value that is not one is refused as
            // a value — at the value, naming the decorator and the key — where
            // `bracketed!` answered syn's `expected square brackets`.
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
                // An entry that is not a type path is the same mistake one
                // level in, and syn's `expected identifier` named nothing.
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
