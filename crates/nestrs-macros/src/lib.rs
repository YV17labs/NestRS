//! The surface-agnostic nestrs decorators: `#[injectable]` (DI provider) and
//! `#[module]` (composition + order-independent registration). Re-exported by
//! `nestrs-core`. Surface-specific decorators live with their surface
//! (`nestrs-http`, `nestrs-graphql`, `nestrs-mcp`); shared token helpers live
//! in `nestrs-codegen`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{
    bracketed, parse_macro_input, Expr, Ident, ImplItem, ItemImpl, ItemStruct, Path, ReturnType,
    Token, Type,
};

use nestrs_codegen::{
    build_injectable_body, dependencies_method, dependency_names_method, from_container_method,
    impl_self_ident, injected_method, optional_dependencies_method, InjectableBody,
};

/// Mark a struct as a provider that can be constructed from the IoC container.
///
/// - Fields tagged `#[inject]` are resolved via `container.get()`.
/// - Other fields fall back to `Default::default()`.
/// - If no field carries `#[inject]`, the macro defers to `<Self as Default>::default()`
///   so any custom `Default` impl on the struct is preserved.
///
/// Also emits `impl Discoverable for Self` so the struct is usable directly
/// in `#[module(providers = [...])]`. The registration simply builds the
/// value via `from_container` and stores it via `ContainerBuilder::provide`.
///
/// `#[injectable(scope = request)]` makes the provider **request-scoped**: it is
/// not built as a singleton but registered as a per-request factory
/// (`ContainerBuilder::provide_scoped`), built fresh for — and cached within —
/// each request, resolved through a `RequestScope` (e.g. the HTTP `Scoped<T>`
/// extractor). Like a controller it is built lazily, so its register-phase
/// `dependencies` are empty while `injected` still reports its `#[inject]` keys
/// for the access-graph check. Its dependencies resolve from the singleton root,
/// so it may inject singletons but not other request-scoped providers. The
/// default, `scope = singleton`, is the plain shared provider.
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    let request_scoped = match parse_injectable_scope(args.into()) {
        Ok(scoped) => scoped,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);

    // A request-scoped provider builds lazily (per request), so — exactly like a
    // controller — it declares no register-phase `dependencies`/ordering and
    // registers a factory rather than a singleton value. `injected` is reported
    // regardless so the access-graph still governs its `#[inject]` keys.
    let (dependencies, dependency_names, optional_dependencies, register_fn) = if request_scoped {
        (
            dependencies_method(&[]),
            dependency_names_method(&[]),
            optional_dependencies_method(&[]),
            quote! {
                fn register(
                    builder: ::nestrs_core::ContainerBuilder,
                ) -> ::nestrs_core::ContainerBuilder {
                    builder.provide_scoped::<Self, _>(|__container| {
                        Self::from_container(__container)
                    })
                }
            },
        )
    } else {
        (
            dependencies_method(&dep_keys),
            dependency_names_method(&dep_names),
            optional_dependencies_method(&opt_keys),
            quote! {
                fn register(
                    builder: ::nestrs_core::ContainerBuilder,
                ) -> ::nestrs_core::ContainerBuilder {
                    let __snapshot = builder.snapshot();
                    let __value = Self::from_container(&__snapshot);
                    builder.provide(__value)
                }
            },
        )
    };

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected

            #register_fn
        }
    }
    .into()
}

/// Parse the optional `#[injectable(scope = …)]` argument. Empty (or
/// `scope = singleton`) is the default singleton provider; `scope = request`
/// marks the provider request-scoped. Returns `true` when request-scoped.
fn parse_injectable_scope(args: TokenStream2) -> syn::Result<bool> {
    if args.is_empty() {
        return Ok(false);
    }
    let parser = |input: ParseStream| -> syn::Result<bool> {
        let key: Ident = input.parse()?;
        if key != "scope" {
            return Err(syn::Error::new(
                key.span(),
                "expected `scope = request` or `scope = singleton`",
            ));
        }
        input.parse::<Token![=]>()?;
        let value: Ident = input.parse()?;
        match value.to_string().as_str() {
            "request" => Ok(true),
            "singleton" => Ok(false),
            other => Err(syn::Error::new(
                value.span(),
                format!("unknown scope `{other}` (expected `request` or `singleton`)"),
            )),
        }
    };
    parser.parse2(args)
}

/// The phase attributes recognised inside a `#[hooks]` impl block, paired with
/// the `LifecyclePhase` variant each maps to.
const HOOK_ATTRS: [(&str, &str); 5] = [
    ("on_module_init", "OnModuleInit"),
    ("on_application_bootstrap", "OnApplicationBootstrap"),
    ("on_module_destroy", "OnModuleDestroy"),
    ("before_application_shutdown", "BeforeApplicationShutdown"),
    ("on_application_shutdown", "OnApplicationShutdown"),
];

/// Declare application lifecycle hooks on a provider's impl block, mirroring
/// NestJS's lifecycle events.
///
/// Applied to an `impl` block of an `#[injectable]` provider. Each method tagged
/// with a phase attribute is invoked by [`App`](nestrs_core::App) at that point:
///
/// - `#[on_module_init]` / `#[on_application_bootstrap]` — after the container
///   is built and transports configured, before serving. An error aborts boot.
/// - `#[on_module_destroy]` / `#[before_application_shutdown]` /
///   `#[on_application_shutdown]` — after the transports stop, best-effort.
///
/// A hook is `async fn(&self)` returning either nothing or
/// `Result<(), E: Into<anyhow::Error>>`. The macro resolves the provider from
/// the container at call time — the same instance request handlers see — and
/// submits each hook to a link-time registry, so there is no central list and
/// the provider keeps its single `impl Discoverable` (emitted by
/// `#[injectable]`). Like the verb attributes of `#[routes]`, the phase
/// attributes are consumed here and need no import.
#[proc_macro_attribute]
pub fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[hooks] takes no arguments; tag methods with `#[on_module_init]`, \
             `#[on_application_shutdown]`, …",
        )
        .to_compile_error()
        .into();
    }

    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();
    let base = match impl_self_ident(&self_ty, "#[hooks]") {
        Ok(base) => base,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_lit = base.to_string();

    let mut submissions: Vec<TokenStream2> = Vec::new();
    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let phase = method.attrs.iter().enumerate().find_map(|(idx, attr)| {
            HOOK_ATTRS
                .iter()
                .find(|(name, _)| attr.path().is_ident(name))
                .map(|(_, variant)| (idx, *variant))
        });
        let Some((idx, phase)) = phase else { continue };
        method.attrs.remove(idx);
        let phase_variant = format_ident!("{}", phase);

        if method.sig.asyncness.is_none() {
            return syn::Error::new_spanned(&method.sig, "#[hooks] methods must be `async fn`")
                .to_compile_error()
                .into();
        }

        let method_name = method.sig.ident.clone();
        let method_lit = method_name.to_string();
        let run_fn = format_ident!("__nestrs_hook_{}_{}", base, method_name);

        // Adapt the method's return to `anyhow::Result<()>`: a bare method is
        // infallible, a returning one must yield `Result<(), E: Into<_>>`.
        let invoke = match &method.sig.output {
            ReturnType::Default => quote! {
                __provider.#method_name().await;
                ::std::result::Result::Ok(())
            },
            ReturnType::Type(..) => quote! {
                ::std::result::Result::map_err(
                    __provider.#method_name().await,
                    ::std::convert::Into::into,
                )
            },
        };

        submissions.push(quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #run_fn(
                __container: &::nestrs_core::Container,
            ) -> ::std::pin::Pin<::std::boxed::Box<
                dyn ::std::future::Future<Output = ::anyhow::Result<()>>
                    + ::std::marker::Send
                    + '_,
            >> {
                ::std::boxed::Box::pin(async move {
                    match ::nestrs_core::Container::get::<#self_ty>(__container) {
                        ::std::option::Option::Some(__provider) => { #invoke }
                        ::std::option::Option::None => ::std::result::Result::Ok(()),
                    }
                })
            }

            ::nestrs_core::inventory::submit! {
                ::nestrs_core::LifecycleHook {
                    phase: ::nestrs_core::LifecyclePhase::#phase_variant,
                    provider: #provider_lit,
                    method: #method_lit,
                    run: #run_fn,
                }
            }
        });
    }

    quote! {
        #item

        #(#submissions)*
    }
    .into()
}

/// `#[module(imports = [...], providers = [...])]`.
///
/// Both keys are optional. `imports` lists other modules to compose in, each
/// contributing its own providers and metadata. An import is either:
///
/// - a **type** (`UsersModule`) — a static [`Module`](nestrs_core::Module),
///   composed via `Module::register`, or
/// - a **call expression** (`OpenApiModule::for_root(opts)`) — a configured
///   [`DynamicModule`](nestrs_core::DynamicModule) value, composed via
///   `DynamicModule::register`. This is how a module receives runtime options
///   at its import site, the analog of NestJS's `forRoot`/`forFeature`.
///
/// `providers` lists everything this module declares — services, controllers,
/// interceptors, cron jobs / event handlers / MCP tools.
///
/// Registration is **idempotent**: the generated `Module::register` marks the
/// module's `TypeId` and returns early if it was already registered, so a
/// module pulled in through several import paths (a diamond) builds its
/// providers exactly once. (Dynamic-module imports carry their own config and
/// are deliberately not deduplicated.)
///
/// Each provider entry is one of:
///
/// - `Foo` — a concrete type that implements `Discoverable` (every
///   `#[injectable]`, `#[controller]`+`#[routes]`, and `#[interceptor]`
///   struct does). The macro expands to a single
///   `<Foo as Discoverable>::register(builder)` call.
/// - `Foo as dyn Trait` — a trait-object binding. The macro builds `Foo`
///   from a snapshot and stores it under the trait's `TypeId` via
///   `provide_dyn`, so dependents can inject `Arc<dyn Trait>`.
///
/// Order does not matter. Imports register first, then providers register by
/// a fixpoint pass: each provider declares its dependencies via
/// `Discoverable::dependencies`, and the macro registers whatever is
/// resolvable, repeating until everything is in. A provider whose
/// dependencies never become available — missing from this module and its
/// imports, or part of a cycle — panics at boot with the offending names.
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ModuleArgs);
    let item = parse_macro_input!(input as ItemStruct);
    let name = item.ident.clone();
    let name_str = name.to_string();

    let import_calls = args.imports.iter().map(|import| match import {
        // A bare type path is a static `Module`, composed by its associated fn.
        Expr::Path(p) => {
            let path = &p.path;
            quote! { builder = <#path as ::nestrs_core::Module>::register(builder); }
        }
        // Anything else — typically `Module::for_root(opts)` — evaluates to a
        // `DynamicModule` value, composed by value.
        other => {
            quote! { builder = ::nestrs_core::DynamicModule::register(#other, builder); }
        }
    });

    // The collect phase mirrors the imports but only queues async factories:
    // static modules recurse via `Module::collect`, dynamic ones via
    // `DynamicModule::collect`. Providers are untouched here.
    let collect_calls = args.imports.iter().map(|import| match import {
        Expr::Path(p) => {
            let path = &p.path;
            quote! { builder = <#path as ::nestrs_core::Module>::collect(builder); }
        }
        other => {
            quote! { builder = ::nestrs_core::DynamicModule::collect(&(#other), builder); }
        }
    });

    // The access-graph descriptor: the bare-type imports
    // and the providers' container keys + declared dependencies, submitted to a
    // link-time registry so `App` can verify at boot that no provider reaches a
    // non-imported module. Only statically-typed imports are recorded — a
    // dynamic `for_root(...)` import contributes only global infrastructure
    // (factory outputs) or self-mounted metadata, never an injectable.
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
        // Per provider, three token streams: the register attempt (hot path), and
        // two cold helpers used only when the fixpoint stalls — its provided key,
        // and a classification of *why* it is still pending.
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
                        // Ready when every required dependency is present and every
                        // optional one is either present or supplied by no remaining
                        // pending provider (so it resolves to `None` rather than
                        // racing a same-module provider — order stays irrelevant).
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
                        // A pure cycle: every dependency this provider still lacks
                        // is one another *pending* provider would supply. Otherwise
                        // a required provider is simply absent.
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
                // Provided keys of providers not yet built this round — lets an
                // optional dependency wait for a same-module provider, and (on a
                // stall) classifies the failure.
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
                    // Stalled: no provider built this round, yet some remain. Tell
                    // the two failure modes apart so the message is actionable.
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
                // Idempotent: a module imported through several paths registers
                // its providers once. Marks before composing imports so a cycle
                // among modules terminates rather than recursing forever.
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
                // Same diamond dedup as `register`, on the collect set, so each
                // module queues its async factories at most once.
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

/// Last path segment as a string (`crate::users::UsersService` -> `"UsersService"`),
/// for readable boot-time panics.
fn path_tail(p: &Path) -> String {
    p.segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| quote!(#p).to_string())
}

/// Last path segment of a trait-object type's path
/// (`dyn crate::weather::WeatherProvider` -> `"WeatherProvider"`), for the
/// access-graph descriptor's human-readable provider label.
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

/// Either a concrete provider (`MyService`) or a trait-object binding
/// (`MyService as dyn MyTrait`). The latter registers the value under the
/// trait's `TypeId` so dependents can inject `Arc<dyn MyTrait>`.
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
