//! `#[mcp]` on an `impl` block — the operations half of the decorator.
//!
//! # What it absorbs
//!
//! rmcp's architecture asks a host for three blocks: `#[tool_router] impl T`,
//! `#[prompt_router] impl T`, and `#[tool_handler] #[prompt_handler] impl
//! ServerHandler for T` with a `get_info` declaring capabilities. Every other
//! edge in this framework is *one* decorated `impl` with decorated methods
//! (`#[routes]`, `#[resolver]`, `#[processor]`, `#[scheduled]`), so MCP was the
//! only one leaking its SDK's shape into the file a developer writes.
//!
//! This expansion takes one authored `impl` and emits all of it: the methods are
//! split by role into rmcp's two routers, the `ServerHandler` impl is generated
//! with the handler attributes it needs, and its capabilities are **derived**
//! from the roles actually present — a host can no longer route tools it forgot
//! to advertise.
//!
//! # Why a private child module
//!
//! rmcp's macros expand to bare `rmcp::` paths resolved against the call site,
//! which is why a host file had to carry `use nest_rs::mcp::rmcp;` — an import
//! whose only job was someone else's hygiene, and which the CLI template shipped
//! with three lines of comment explaining it. Emitting the impls inside a
//! private module lets *that* module carry the imports instead.
//!
//! The Rust fact that makes it sound is pinned by
//! `tests/integration/mcp_impl.rs`: an inherent impl may be written in any
//! module of the defining crate, and a descendant still reaches the parent's
//! private fields.
//!
//! Visibility is the wrinkle. rmcp generates `tool_router()` without `pub`, so
//! the router would die at this module's edge and the duplicate-tool boot check
//! — which reads it from the parent — would silently see an empty list. rmcp
//! answers that itself: `#[tool_router(vis = "…")]` sets the generated
//! function's visibility, so asking for `pub(crate)` is all this needs. An
//! earlier draft grew a second accessor and a second fallback trait to work
//! around a problem the SDK had already solved.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{Attribute, ImplItem, ImplItemFn, ItemImpl, Meta, Token};

use nest_rs_codegen::{expr_str, impl_self_ident};

/// The decorated methods of one authored `impl`, already partitioned by the
/// router each belongs to. Holding the split once is what lets every downstream
/// question — which routers to emit, which handler attributes, which
/// capabilities — be a plain `is_empty()`.
#[derive(Default)]
struct Operations {
    tools: Vec<ImplItemFn>,
    prompts: Vec<ImplItemFn>,
}

/// Which router a decorated method belongs to.
#[derive(Clone, Copy)]
enum Role {
    Tool,
    Prompt,
}

impl Role {
    /// The role an attribute gives a method, if it gives one.
    fn from_attr(attr: &Attribute) -> Option<Self> {
        match () {
            () if attr.path().is_ident("tool") => Some(Self::Tool),
            () if attr.path().is_ident("prompt") => Some(Self::Prompt),
            _ => None,
        }
    }
}

pub(crate) fn mcp_impl(args: TokenStream, item: ItemImpl) -> TokenStream {
    if let Err(err) = reject_args(&args) {
        return err.to_compile_error().into();
    }
    match expand(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The impl-level `#[mcp]` takes nothing: what the endpoint *is* is declared on
/// the struct, and what each operation does on its own method.
fn reject_args(args: &TokenStream) -> syn::Result<()> {
    if args.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "#[mcp] on an impl block takes no arguments — the endpoint's path and \
         identity are declared on the struct, and each operation is described by \
         its own doc comment",
    ))
}

fn expand(item: ItemImpl) -> syn::Result<TokenStream2> {
    if let Some((_, path)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[mcp] decorates the inherent impl that holds a host's #[tool] and \
             #[prompt] methods. A hand-written `impl ServerHandler` stays as it \
             is — reach for rmcp directly there",
        ));
    }

    let operations = take_operations(&item)?;
    if operations.tools.is_empty() && operations.prompts.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[mcp] on an impl with no #[tool] or #[prompt] method has nothing to \
             mount — drop the decorator, or mark the methods it should serve",
        ));
    }

    let self_ty = &item.self_ty;
    let module = format_ident!(
        "__nest_rs_mcp_{}",
        nest_rs_codegen::snake_case(&impl_self_ident(self_ty, "#[mcp]")?.to_string()),
    );
    let generics = item.generics.split_for_impl();

    // `vis = "pub(crate)"` is what carries rmcp's generated router out of this
    // module, so the boot checks can still read the host's tool names.
    let tool_impl = router_impl(
        self_ty,
        &generics,
        &operations.tools,
        quote!(tool_router(vis = "pub(crate)")),
    );
    let prompt_impl = router_impl(
        self_ty,
        &generics,
        &operations.prompts,
        quote!(prompt_router),
    );

    let handler_attrs = {
        let tools = (!operations.tools.is_empty()).then(|| quote!(#[tool_handler]));
        let prompts = (!operations.prompts.is_empty()).then(|| quote!(#[prompt_handler]));
        quote!(#tools #prompts)
    };

    // Capabilities are *derived*, never restated: a router with methods in it is
    // the proof the surface exists, so a host cannot route tools it forgot to
    // advertise. A hand-written surface (resources, completion) is declared by
    // hand in its own `impl ServerHandler`, which is the one shape this does not
    // generate.
    let capabilities = {
        let tools = (!operations.tools.is_empty()).then(|| quote!(.enable_tools()));
        let prompts = (!operations.prompts.is_empty()).then(|| quote!(.enable_prompts()));
        quote!(ServerCapabilities::builder() #tools #prompts .build())
    };

    let (impl_generics, ty_generics, where_clause) = &generics;
    Ok(quote! {
        #[doc(hidden)]
        mod #module {
            use super::*;

            // The names rmcp's own expansions resolve against — the import that
            // used to sit in the developer's file, now scoped to generated code.
            use ::nest_rs_mcp::rmcp;
            use ::nest_rs_mcp::model::{ServerCapabilities, ServerInfo};
            use ::nest_rs_mcp::{
                ServerHandler, prompt, prompt_handler, prompt_router, tool, tool_handler,
                tool_router,
            };

            #tool_impl
            #prompt_impl

            #handler_attrs
            impl #impl_generics ServerHandler for #self_ty #ty_generics #where_clause {
                fn get_info(&self) -> ServerInfo {
                    ServerInfo::new(#capabilities)
                }
            }
        }
    })
}

/// One rmcp router impl carrying `methods`, or nothing when the host serves none
/// of that role.
fn router_impl(
    self_ty: &syn::Type,
    (impl_generics, ty_generics, where_clause): &Generics<'_>,
    methods: &[ImplItemFn],
    router: TokenStream2,
) -> Option<TokenStream2> {
    if methods.is_empty() {
        return None;
    }
    Some(quote! {
        #[#router]
        impl #impl_generics #self_ty #ty_generics #where_clause {
            #(#methods)*
        }
    })
}

/// The three halves of `Generics::split_for_impl`, computed once by `expand` and
/// handed down rather than re-derived per router.
type Generics<'a> = (
    syn::ImplGenerics<'a>,
    syn::TypeGenerics<'a>,
    Option<&'a syn::WhereClause>,
);

/// Partition the decorated methods by role, giving each one the description its
/// doc comment already carries.
fn take_operations(item: &ItemImpl) -> syn::Result<Operations> {
    let mut operations = Operations::default();

    for entry in &item.items {
        let ImplItem::Fn(method) = entry else {
            return Err(unsupported(entry));
        };
        let Some((index, role)) = method
            .attrs
            .iter()
            .enumerate()
            .find_map(|(index, attr)| Role::from_attr(attr).map(|role| (index, role)))
        else {
            // Helpers belong beside the struct: left here they would move into
            // the generated module, where a reader would not look for them.
            return Err(unsupported(entry));
        };

        let mut method = method.clone();
        fill_description(&mut method, index)?;
        match role {
            Role::Tool => operations.tools.push(method),
            Role::Prompt => operations.prompts.push(method),
        }
    }

    Ok(operations)
}

/// The one thing an authored `#[mcp] impl` may not hold, spanned on the item
/// itself rather than on the type — a reader needs to see *which* one.
fn unsupported(entry: &ImplItem) -> syn::Error {
    syn::Error::new_spanned(
        entry,
        "#[mcp] serves only #[tool] and #[prompt] methods — move anything else to \
         a plain `impl` block beside it",
    )
}

/// Give `#[tool]` / `#[prompt]` the description the method's doc comment
/// already states, unless the attribute wrote one itself.
///
/// The prose was being written twice — once for the reader, once for the model —
/// and two copies of one sentence drift. The doc comment is the copy that a
/// reader of the source sees, so it is the one that wins.
fn fill_description(method: &mut ImplItemFn, index: usize) -> syn::Result<()> {
    if states_description(&method.attrs[index])? {
        return Ok(());
    }
    let Some(doc) = doc_comment(&method.attrs) else {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "an MCP operation needs a description — the model reads it to choose \
             between operations. Write a doc comment above it, or state \
             `description = \"…\"` on the attribute",
        ));
    };

    let attr = &method.attrs[index];
    let role = attr.path().clone();
    let described = match &attr.meta {
        // `#[tool]` — nothing else stated.
        Meta::Path(_) => quote!(#[#role(description = #doc)]),
        // `#[tool(annotations(..))]` and friends keep what they wrote.
        Meta::List(list) => {
            let rest = &list.tokens;
            quote!(#[#role(description = #doc, #rest)])
        }
        Meta::NameValue(value) => {
            return Err(syn::Error::new_spanned(
                value,
                "expected `#[tool]` or `#[tool(...)]`",
            ));
        }
    };
    let mut parsed = syn::parse::Parser::parse2(Attribute::parse_outer, described)?;
    method.attrs[index] = parsed.remove(0);
    Ok(())
}

/// Whether the attribute already states its own `description`.
///
/// Parsing the arguments as a `Meta` list is what keeps every other key rmcp
/// accepts intact: `Meta::List` captures a nested group like
/// `annotations(title = "…", read_only_hint = true)` as opaque tokens without
/// descending into it, so nothing here has to know rmcp's grammar.
fn states_description(attr: &Attribute) -> syn::Result<bool> {
    let Meta::List(_) = &attr.meta else {
        return Ok(false);
    };
    let args = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    Ok(args.iter().any(|meta| meta.path().is_ident("description")))
}

/// The method's doc comment, joined into the one sentence rmcp sends.
fn doc_comment(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(value) => expr_str(&value.value).ok(),
            _ => None,
        })
        .map(|literal| literal.value().trim().to_owned())
        .collect();

    let joined = lines.join(" ");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
