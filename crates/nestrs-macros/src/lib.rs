//! Procedural macros mirroring NestJS's `@Injectable`, `@Module`, `@Controller`
//! and the per-method route decorators (`@Get`, `@Post`, ...).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    bracketed, parse_macro_input, Fields, FnArg, GenericArgument, Ident, ImplItem, ItemImpl,
    ItemStruct, LitStr, Pat, Path, PathArguments, ReturnType, Token, Type,
};

// -----------------------------------------------------------------------------
// #[injectable]
// -----------------------------------------------------------------------------

/// Mark a struct as a provider that can be constructed from the IoC container.
///
/// - Fields tagged `#[inject]` are resolved via `container.get()`.
/// - Other fields fall back to `Default::default()`.
/// - If no field carries `#[inject]`, the macro defers to `<Self as Default>::default()`
///   so any custom `Default` impl on the struct is preserved.
#[proc_macro_attribute]
pub fn injectable(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemStruct);

    let body = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            pub fn from_container(container: &::nestrs_core::Container) -> Self {
                let _ = container;
                #body
            }
        }
    }
    .into()
}

/// If `ty` syntactically matches `Arc<dyn Trait + ...>`, return the inner
/// trait-object type so the macro can emit a `get_dyn::<dyn Trait + ...>()`
/// call. Only the last path segment is inspected (`std::sync::Arc<dyn T>`
/// works as well as `Arc<dyn T>`).
fn arc_dyn_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Arc" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    let GenericArgument::Type(inner) = &args.args[0] else {
        return None;
    };
    matches!(inner, Type::TraitObject(_)).then_some(inner)
}

fn build_injectable_body(item: &mut ItemStruct) -> syn::Result<TokenStream2> {
    match &mut item.fields {
        Fields::Unit => Ok(quote! { Self }),
        Fields::Named(fields) => {
            let mut has_inject = false;
            let mut field_inits = Vec::new();

            for field in fields.named.iter_mut() {
                let field_name = field.ident.clone().expect("named field has an ident");
                let inject_idx = field.attrs.iter().position(|a| a.path().is_ident("inject"));
                if let Some(idx) = inject_idx {
                    field.attrs.remove(idx);
                    has_inject = true;
                    let msg = format!(
                        "{}.{}: no provider registered for this dependency",
                        item.ident, field_name
                    );
                    if let Some(trait_ty) = arc_dyn_inner(&field.ty) {
                        field_inits.push(quote! {
                            #field_name: container.get_dyn::<#trait_ty>().expect(#msg)
                        });
                    } else {
                        field_inits.push(quote! {
                            #field_name: container.get().expect(#msg)
                        });
                    }
                } else {
                    field_inits.push(quote! {
                        #field_name: ::core::default::Default::default()
                    });
                }
            }

            if has_inject {
                Ok(quote! { Self { #(#field_inits),* } })
            } else {
                Ok(quote! { <Self as ::core::default::Default>::default() })
            }
        }
        Fields::Unnamed(_) => Err(syn::Error::new_spanned(
            &item.fields,
            "#[injectable] does not support tuple structs",
        )),
    }
}

// -----------------------------------------------------------------------------
// #[module]
// -----------------------------------------------------------------------------

/// Declare a feature module — equivalent of `@Module({ imports, providers })` in NestJS.
///
/// Syntax: `#[module(imports = [OtherModule], providers = [SomeService])]`.
/// Both keys are optional. Providers are registered in order against a fresh
/// snapshot of the container so a later provider can depend on an earlier one.
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ModuleArgs);
    let item = parse_macro_input!(input as ItemStruct);
    let name = item.ident.clone();

    let import_calls = args.imports.iter().map(|p| {
        quote! { builder = <#p as ::nestrs_core::Module>::register(builder); }
    });

    let provider_calls = args.providers.iter().map(|binding| match binding {
        ProviderBinding::Concrete(p) => quote! {
            {
                let __snapshot = builder.snapshot();
                let __provider = #p::from_container(&__snapshot);
                builder = builder.provide(__provider);
            }
        },
        ProviderBinding::Dyn { provider, trait_ty } => quote! {
            {
                let __snapshot = builder.snapshot();
                let __provider = #provider::from_container(&__snapshot);
                let __dyn: ::std::sync::Arc<#trait_ty> =
                    ::std::sync::Arc::new(__provider);
                builder = builder.provide_dyn::<#trait_ty>(__dyn);
            }
        },
    });

    quote! {
        #item

        impl ::nestrs_core::Module for #name {
            fn register(
                mut builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                #(#import_calls)*
                #(#provider_calls)*
                builder
            }
        }
    }
    .into()
}

#[derive(Default)]
struct ModuleArgs {
    imports: Vec<Path>,
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
                    let paths: Punctuated<Path, Token![,]> =
                        Punctuated::parse_terminated(&content)?;
                    args.imports.extend(paths);
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

// -----------------------------------------------------------------------------
// #[controller(path = "...")]
// -----------------------------------------------------------------------------

/// Declare an HTTP controller — equivalent of `@Controller('/health')` in NestJS.
///
/// Generates:
/// - A `from_container(&Container) -> Self` constructor (like `#[injectable]`).
/// - A `pub const PATH: &'static str` used by `#[routes]` as the route prefix.
///
/// Use together with `#[routes]` on the corresponding `impl` block.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ControllerArgs);
    let mut item = parse_macro_input!(input as ItemStruct);

    let body = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let path_lit = args.path;

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            pub const PATH: &'static str = #path_lit;

            pub fn from_container(container: &::nestrs_core::Container) -> Self {
                let _ = container;
                #body
            }
        }
    }
    .into()
}

struct ControllerArgs {
    path: LitStr,
}

impl Parse for ControllerArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        if key != "path" {
            return Err(syn::Error::new(
                key.span(),
                "expected `path = \"...\"` as the only #[controller] argument",
            ));
        }
        input.parse::<Token![=]>()?;
        let path: LitStr = input.parse()?;
        Ok(ControllerArgs { path })
    }
}

// -----------------------------------------------------------------------------
// #[routes]
// -----------------------------------------------------------------------------

/// Bind controller methods to HTTP routes.
///
/// Applied to an `impl` block belonging to a `#[controller]`-marked struct.
/// Each method tagged with `#[get("/path")]`, `#[post("/path")]`, `#[put]`,
/// `#[delete]` or `#[patch]` is wired as a poem handler. Method signatures
/// keep `&self` plus any poem extractors (`Path<T>`, `Json<T>`, `Query<T>`...).
///
/// Generates a `routes(container: &Container) -> impl IntoEndpoint` associated
/// function on the controller, already nested under the controller's `PATH`.
#[proc_macro_attribute]
pub fn routes(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();

    let mut wrappers: Vec<TokenStream2> = Vec::new();
    let mut route_entries: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let verb_idx = method.attrs.iter().position(|attr| {
            ["get", "post", "put", "delete", "patch"]
                .iter()
                .any(|v| attr.path().is_ident(v))
        });
        let Some(idx) = verb_idx else { continue };

        let attr = method.attrs.remove(idx);
        let verb_ident = attr
            .path()
            .get_ident()
            .expect("verb attribute has an ident")
            .clone();

        let route_path: LitStr = match attr.parse_args() {
            Ok(p) => p,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_name = method.sig.ident.clone();
        let wrapper_name = format_ident!("__nestrs_route_{}", method_name);

        let inputs: Vec<FnArg> = method.sig.inputs.iter().skip(1).cloned().collect();
        let arg_idents: Vec<Ident> = inputs
            .iter()
            .filter_map(|arg| match arg {
                FnArg::Typed(pt) => match &*pt.pat {
                    Pat::Ident(pi) => Some(pi.ident.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        let return_type = match &method.sig.output {
            ReturnType::Default => quote! { () },
            ReturnType::Type(_, ty) => quote! { #ty },
        };

        let extra_inputs = if inputs.is_empty() {
            quote! {}
        } else {
            quote! { , #(#inputs),* }
        };

        wrappers.push(quote! {
            #[::poem::handler]
            async fn #wrapper_name(
                ::poem::web::Data(__ctrl): ::poem::web::Data<&::std::sync::Arc<#self_ty>>
                #extra_inputs
            ) -> #return_type {
                __ctrl.#method_name(#(#arg_idents),*).await
            }
        });

        route_entries.push(quote! {
            .at(#route_path, ::poem::#verb_ident(#wrapper_name))
        });
    }

    quote! {
        #item

        #(#wrappers)*

        impl #self_ty {
            pub fn routes(
                container: &::nestrs_core::Container,
            ) -> impl ::poem::IntoEndpoint {
                use ::poem::EndpointExt;
                let __ctrl = ::std::sync::Arc::new(<#self_ty>::from_container(container));
                ::poem::Route::new().nest(
                    <#self_ty>::PATH,
                    ::poem::Route::new()
                        #(#route_entries)*
                        .data(__ctrl),
                )
            }
        }
    }
    .into()
}
