//! The resource roles of `#[tools]`: a `#[resource]` read through an RFC 6570
//! template whose variables are its typed arguments, and the `#[resources]`
//! listing. rmcp routes neither, so the expansion writes the three
//! `ServerHandler` methods that serve them itself.

use nest_rs_codegen::__private::{PipeWrapper, generic_args, pipe_wrapper};
use nest_rs_codegen::Grammar;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Attribute, Expr, FnArg, ImplItemFn, LitStr, Meta, Signature, Type};

use crate::mcp_impl::{Description, description_check, doc_comment, tail_param};
use crate::uri_template::{self, Part};

/// What `#[resource(…)]` takes.
const RESOURCE: Grammar = Grammar::new("resource", &["template", "name", "title", "description"]);

/// What `#[resources]` takes: nothing, its entries naming themselves.
const RESOURCES: Grammar = Grammar::new("resources", &[]);

/// What one `#[resource]` declares beside the method that reads it.
pub(crate) struct ResourceDecl {
    /// The template as written, for its span and its advertisement.
    pub(crate) template: LitStr,
    pub(crate) parts: Vec<Part>,
    /// The template's programmatic name: the method's, unless stated.
    pub(crate) name: String,
    pub(crate) title: Option<Expr>,
    /// The prose a client reads, stated or lifted from the doc comment.
    pub(crate) description: TokenStream2,
    /// A `const` refusing a description only the compiler can read blank,
    /// taken by the operation that emits every role's.
    pub(crate) description_check: Option<TokenStream2>,
    /// Each parameter a variable fills, in the method's order.
    pub(crate) variables: Vec<VariableArg>,
}

/// A parameter a template variable fills, read off the URI with `FromStr`.
pub(crate) struct VariableArg {
    pub(crate) ident: syn::Ident,
    pub(crate) ty: Type,
    /// Its variable's position among the template's.
    pub(crate) index: usize,
}

impl ResourceDecl {
    /// The variables, in the template's order.
    fn variable_names(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(|part| match part {
            Part::Variable(name) => Some(name.as_str()),
            Part::Literal(_) => None,
        })
    }
}

/// Read `#[resource(…)]` — the method's `attrs[index]` — before it comes off.
pub(crate) fn declaration(method: &ImplItemFn, index: usize) -> syn::Result<ResourceDecl> {
    let attr = &method.attrs[index];
    let mut template = None;
    let mut name = None;
    let mut title = None;
    let mut description = None;
    match &attr.meta {
        Meta::List(_) => RESOURCE.parse_attr(attr, |arg| {
            match arg.key() {
                "template" => template = Some(arg.str_lit("post://{id}")?),
                "name" => name = Some(arg.str_lit("post")?.value()),
                "title" => title = Some(arg.expr()?),
                "description" => description = Some(arg.expr()?),
                // The grammar hands over only its own keys.
                _ => {}
            }
            Ok(())
        })?,
        Meta::NameValue(value) => {
            return Err(syn::Error::new_spanned(
                value,
                "expected `#[resource(template = \"…\")]`",
            ));
        }
        Meta::Path(_) => {}
    }
    let Some(template) = template else {
        return Err(syn::Error::new_spanned(
            attr,
            "#[resource] needs a `template` — the URI template it is read at, e.g. \
             `#[resource(template = \"post://{id}\")]`",
        ));
    };
    let parts = uri_template::parse(&template)?;
    let (description, description_check) = described(method, description)?;
    Ok(ResourceDecl {
        template,
        parts,
        name: name.unwrap_or_else(|| method.sig.ident.to_string()),
        title,
        description,
        description_check,
        variables: Vec::new(),
    })
}

/// Refuse any argument `#[resources]` is written with.
pub(crate) fn listing_declaration(attr: &Attribute) -> syn::Result<()> {
    match &attr.meta {
        Meta::Path(_) => Ok(()),
        Meta::List(_) => RESOURCES.parse_attr(attr, |_| Ok(())),
        Meta::NameValue(value) => Err(syn::Error::new_spanned(
            value,
            "expected `#[resources]` — a listing takes no argument, each entry naming itself",
        )),
    }
}

/// The description a client reads — stated, else the doc comment — and the
/// `const` refusing one only the compiler can read blank.
fn described(
    method: &ImplItemFn,
    stated: Option<Expr>,
) -> syn::Result<(TokenStream2, Option<TokenStream2>)> {
    let (tokens, description) = match stated {
        Some(expr) => (quote!(#expr), Some(Description::stated(&expr)?)),
        None => {
            let doc = doc_comment(&method.attrs);
            (quote!(#doc), doc.map(Description::read))
        }
    };
    Ok((tokens, description_check(description, method)?))
}

/// Pair each parameter of the normalized `sig` with the template variable that
/// fills it, refusing a parameter no variable names, one wrapped as a tool's
/// argument is, and a variable no parameter takes.
pub(crate) fn bind_variables(decl: &mut ResourceDecl, sig: &Signature) -> syn::Result<()> {
    let mut variables = Vec::new();
    for input in &sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            continue;
        };
        let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
            continue;
        };
        if tail_param(&pat_type.ty).is_some() {
            continue;
        }
        let ident = &pat_ident.ident;
        let index = decl.variable_names().position(|name| ident == name);
        if index.is_some()
            && let Some(carrier) = argument_carrier(&pat_type.ty)
        {
            return Err(syn::Error::new_spanned(
                pat_type,
                format!(
                    "#[resource] reads `{{{ident}}}` off its URI with `FromStr`: write \
                     `{ident}: T`, not `{carrier}<…>` — `Parameters`, `Valid` and `Piped` \
                     carry a tool's JSON arguments, which a URI does not send",
                ),
            ));
        }
        let Some(index) = index else {
            return Err(syn::Error::new_spanned(
                pat_type,
                format!(
                    "#[resource] reads its arguments off its URI template: `{ident}` names no \
                     variable of `{template}` — name it after one (`{{id}}` is read into \
                     `id: T`, where `T: FromStr`), or take a request-scoped provider as \
                     `Scoped<T>` or what a guard attached as `Ctx<T>`",
                    template = decl.template.value(),
                ),
            ));
        };
        variables.push(VariableArg {
            ident: ident.clone(),
            ty: (*pat_type.ty).clone(),
            index,
        });
    }
    if let Some(unread) = decl
        .variable_names()
        .find(|name| !variables.iter().any(|arg| arg.ident == name))
    {
        return Err(syn::Error::new_spanned(
            &decl.template,
            format!(
                "#[resource] `template` names `{{{unread}}}`, which no parameter of `{method}` \
                 takes — add `{unread}: T`, where `T: FromStr` reads it off the URI",
                method = sig.ident,
            ),
        ));
    }
    decl.variables = variables;
    Ok(())
}

/// The wrapper a tool's argument travels in — `Parameters`, `Valid`,
/// `Piped` — when `ty` is spelled in one.
fn argument_carrier(ty: &Type) -> Option<&'static str> {
    if generic_args(ty).is_some_and(|(name, _)| name == "Parameters") {
        return Some("Parameters");
    }
    pipe_wrapper(ty).map(|wrapper| match wrapper {
        PipeWrapper::Valid { .. } => "Valid",
        PipeWrapper::Piped { .. } => "Piped",
    })
}

/// Refuse a parameter of a `#[resources]` listing other than a `Scoped<T>` or a
/// `Ctx<T>`.
pub(crate) fn refuse_listing_arguments(sig: &Signature) -> syn::Result<()> {
    for input in &sig.inputs {
        if let FnArg::Typed(pat_type) = input
            && tail_param(&pat_type.ty).is_none()
        {
            return Err(syn::Error::new_spanned(
                pat_type,
                "#[resources] lists every resource a caller may read, so it takes no \
                 argument but a request-scoped provider (`Scoped<T>`) or what a guard \
                 attached (`Ctx<T>`) — a resource read by its own arguments is a \
                 `#[resource(template = \"…\")]`",
            ));
        }
    }
    Ok(())
}

/// The `UriTemplate` the template splits into, as the const the host declares.
pub(crate) fn template_const(decl: &ResourceDecl) -> TokenStream2 {
    let template = &decl.template;
    let parts = decl.parts.iter().map(|part| match part {
        Part::Literal(text) => quote!(::nest_rs_mcp::__private::TemplatePart::Literal(#text)),
        Part::Variable(name) => quote!(::nest_rs_mcp::__private::TemplatePart::Variable(#name)),
    });
    quote! {
        ::nest_rs_mcp::__private::UriTemplate::new(#template, &[#(#parts),*])
    }
}

/// The `ResourceTemplate` `resources/templates/list` advertises for `decl`,
/// whose method answers `answer`.
pub(crate) fn advertised(decl: &ResourceDecl, answer: &syn::ReturnType) -> TokenStream2 {
    let ResourceDecl {
        template,
        name,
        title,
        description,
        ..
    } = decl;
    let title = title
        .as_ref()
        .map(|title| quote!(let __nestrs_template = __nestrs_template.with_title(#title);));
    // A probe, so an answer the read refuses is refused there alone.
    let mime = match answer {
        syn::ReturnType::Type(_, ty) => quote! {{
            #[allow(
                unused_imports,
                reason = "the probe's two arms are in scope for autoref, which no lint sees"
            )]
            use ::nest_rs_mcp::__private::{MimeProbeFallback as _, MimeProbeReads as _};
            (&&::nest_rs_mcp::__private::MimeProbe::<#ty>::new()).mime_type()
        }},
        syn::ReturnType::Default => quote!(::core::option::Option::None),
    };
    quote! {{
        let __nestrs_template =
            ::nest_rs_mcp::model::ResourceTemplate::new(#template, #name)
                .with_description(#description);
        #title
        match #mime {
            ::core::option::Option::Some(__nestrs_mime) => {
                __nestrs_template.with_mime_type(__nestrs_mime)
            }
            ::core::option::Option::None => __nestrs_template,
        }
    }}
}

/// One template's turn in the generated `read_resource`: when `uri` is one the
/// template writes and every variable parses as its parameter's type, the
/// read is the method's, through its `wrapper`.
pub(crate) fn read_arm(
    decl: &ResourceDecl,
    template: &syn::Ident,
    wrapper: &syn::Ident,
    cfgs: &[TokenStream2],
) -> TokenStream2 {
    let read = quote!(<Self>::#template.read(&__nestrs_uri));
    if decl.variables.is_empty() {
        return quote! {
            #(#cfgs)*
            let () = if #read.is_some() {
                return self.#wrapper(__nestrs_uri).await.map(::core::convert::Into::into);
            };
        };
    }
    let idents: Vec<syn::Ident> = decl
        .variables
        .iter()
        .map(|arg| format_ident!("__nestrs_var_{}", arg.ident))
        .collect();
    let parsed = decl.variables.iter().map(|VariableArg { ty, index, .. }| {
        quote_spanned! {ty.span()=> __nestrs_read.variable::<#ty>(#index)?}
    });
    quote! {
        #(#cfgs)*
        let () = if let ::core::option::Option::Some((#(#idents,)*)) =
            #read.and_then(|__nestrs_read| ::core::option::Option::Some((#(#parsed,)*)))
        {
            return self
                .#wrapper(__nestrs_uri, #(#idents),*)
                .await
                .map(::core::convert::Into::into);
        };
    }
}
