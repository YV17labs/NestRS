//! `#[crud]` — synthesise the standard resolver operations the developer did
//! not hand-write, then re-emit under `#[resolver]`. Every operation
//! delegates to the entity's [`CrudService`] — never `Repo` directly.
//! Override by writing the matching method.
//!
//! Each generated operation declares its posture with the same
//! `#[authorize(Action, Entity)]` a hand-written one would — `#[resolver]`
//! emits the class gate and the response mask from it, so generated and
//! hand-written operations share one mechanism. The by-id operations
//! (`get`/`update`/`delete`) still row-gate through [`CrudService::access`];
//! the class gate in front of it is observably equivalent for any caller with
//! at least one grant (`Ability::can_class` counts row-scoped rules) and
//! rejects zero-grant callers one step earlier.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, Ident, ImplItem, ItemImpl, Pat, Signature, Stmt, Type, parse_macro_input,
    parse_quote,
};

use nest_rs_codegen::{Paginate, nth_generic_type, parse_crud_args, singular_of};

pub(crate) fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    match crud(TokenStream2::from(args), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn crud(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let cfg = parse_crud_args(args)?;
    let ops = cfg.generated_ops()?;

    let existing: HashSet<String> = item
        .items
        .iter()
        .filter_map(|it| match it {
            ImplItem::Fn(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect();

    let service = &cfg.service;
    let entity = &cfg.entity;
    let output = &cfg.output;
    let singular = singular_of(output);
    let list_op = format_ident!("{}s", singular);
    let get_op = format_ident!("{}", singular);
    let create_op = format_ident!("create_{}", singular);
    let update_op = format_ident!("update_{}", singular);
    let delete_op = format_ident!("delete_{}", singular);

    // Validation half of route-model binding (bad format/version => GraphQL
    // error before any load); the load + authz half is the service's `access`.
    let parse_id: TokenStream2 = quote! {
        let __id = ::uuid::Uuid::parse_str(&id)
            .map_err(|__e| ::nest_rs_graphql::async_graphql::Error::new(
                ::std::string::ToString::to_string(&__e),
            ))?;
        if __id.get_version_num() != 7 {
            return ::core::result::Result::Err(
                ::nest_rs_graphql::async_graphql::Error::new("id must be a UUID v7"),
            );
        }
    };
    let gql_err: TokenStream2 = quote! {
        |__e| ::nest_rs_graphql::async_graphql::Error::new(::std::string::ToString::to_string(&__e))
    };
    let forbidden: TokenStream2 = quote! {
        ::nest_rs_graphql::async_graphql::Error::new("forbidden")
    };

    let mut generated: Vec<ImplItem> = Vec::new();

    if ops.list && !existing.contains(&list_op.to_string()) {
        let list_method: ImplItem = match cfg.paginate {
            // Keyset pagination (the default): `first` capped by
            // `clamp_page_size`, `after` = the last item's id (UUID-v7 keys
            // are time-ordered, so the cursor is just the previous page's
            // last `id`). The body stays a plain `Vec` so the automatic
            // response mask applies unchanged.
            Paginate::Cursor => parse_quote! {
                #[query]
                #[authorize(::nest_rs_authz::Read, #entity)]
                async fn #list_op(
                    &self,
                    first: ::core::option::Option<u64>,
                    after: ::core::option::Option<::std::string::String>,
                ) -> ::nest_rs_graphql::async_graphql::Result<::std::vec::Vec<#output>> {
                    let __after = after
                        .as_deref()
                        .and_then(|__s| ::uuid::Uuid::parse_str(__s).ok());
                    let __page = ::nest_rs_seaorm::CrudService::page(
                        &*self.#service,
                        ::core::option::Option::unwrap_or(first, 20),
                        __after,
                    )
                    .await
                    .map_err(#gql_err)?;
                    ::core::result::Result::Ok(
                        __page.items.iter().map(#output::from).collect(),
                    )
                }
            },
            // Explicit opt-out (`paginate = none`): the full ability-scoped
            // collection, still backstopped by `CrudService::list`'s hard cap.
            Paginate::None => parse_quote! {
                #[query]
                #[authorize(::nest_rs_authz::Read, #entity)]
                async fn #list_op(
                    &self,
                ) -> ::nest_rs_graphql::async_graphql::Result<::std::vec::Vec<#output>> {
                    let __rows = ::nest_rs_seaorm::CrudService::list(&*self.#service)
                        .await
                        .map_err(#gql_err)?;
                    ::core::result::Result::Ok(__rows.iter().map(#output::from).collect())
                }
            },
            Paginate::Page => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "#[crud] GraphQL list does not yet support `paginate = page` (offset); \
                     use `paginate = cursor` (the default) or `paginate = none`",
                ));
            }
        };
        generated.push(list_method);
    }

    if ops.get && !existing.contains(&get_op.to_string()) {
        generated.push(parse_quote! {
            #[query]
            #[authorize(::nest_rs_authz::Read, #entity)]
            async fn #get_op(
                &self,
                id: ::std::string::String,
            ) -> ::nest_rs_graphql::async_graphql::Result<::core::option::Option<#output>> {
                #parse_id
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Read,
                    __id,
                )
                .await
                .map_err(#gql_err)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => ::core::result::Result::Ok(
                        ::core::option::Option::Some(#output::from(&__m)),
                    ),
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(#forbidden),
                    ::nest_rs_seaorm::Access::Missing => {
                        ::core::result::Result::Ok(::core::option::Option::None)
                    }
                }
            }
        });
    }

    if let Some(create) = ops.create
        && !existing.contains(&create_op.to_string())
    {
        generated.push(parse_quote! {
            #[mutation]
            #[authorize(::nest_rs_authz::Create, #entity)]
            async fn #create_op(
                &self,
                input: #create,
            ) -> ::nest_rs_graphql::async_graphql::Result<#output> {
                let __row = ::nest_rs_seaorm::Creatable::create(&*self.#service, input)
                    .await
                    .map_err(#gql_err)?;
                ::core::result::Result::Ok(#output::from(&__row))
            }
        });
    }

    if let Some(update) = ops.update
        && !existing.contains(&update_op.to_string())
    {
        generated.push(parse_quote! {
            #[mutation]
            #[authorize(::nest_rs_authz::Update, #entity)]
            async fn #update_op(
                &self,
                id: ::std::string::String,
                input: #update,
            ) -> ::nest_rs_graphql::async_graphql::Result<::core::option::Option<#output>> {
                #parse_id
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Update,
                    __id,
                )
                .await
                .map_err(#gql_err)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        let __row = ::nest_rs_seaorm::Updatable::update(
                            &*self.#service,
                            __m,
                            input,
                        )
                        .await
                        .map_err(#gql_err)?;
                        ::core::result::Result::Ok(::core::option::Option::Some(
                            #output::from(&__row),
                        ))
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(#forbidden),
                    ::nest_rs_seaorm::Access::Missing => {
                        ::core::result::Result::Ok(::core::option::Option::None)
                    }
                }
            }
        });
    }

    if ops.delete && !existing.contains(&delete_op.to_string()) {
        generated.push(parse_quote! {
            #[mutation]
            #[authorize(::nest_rs_authz::Delete, #entity)]
            async fn #delete_op(
                &self,
                id: ::std::string::String,
            ) -> ::nest_rs_graphql::async_graphql::Result<bool> {
                #parse_id
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Delete,
                    __id,
                )
                .await
                .map_err(#gql_err)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::nest_rs_seaorm::Deletable::delete(&*self.#service, __m)
                            .await
                            .map_err(#gql_err)?;
                        ::core::result::Result::Ok(true)
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(#forbidden),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Ok(false),
                }
            }
        });
    }

    // Signature-only mutations: a hand-written `#[mutation]` whose subject is an
    // `Authorized<E, A>` parameter and which declares no `#[authorize]`/`#[public]`
    // posture. The parameter type *is* the policy — `#[crud]` reads the entity +
    // action off it and the service off `#[crud(service = …)]`, then synthesises
    // the same binding the explicit `#[authorize(A, bind = Service)]` form does:
    // it adds `#[authorize(A, E)]` (so `#[resolver]` emits the class gate + the
    // response mask) and replaces the subject with a by-id argument bound through
    // `bind_required_with(&*self.<service>, …)`. No service type retyped, no id
    // parsing, no raw ORM — and the action carried in the proof type is the one
    // the gate enforces, checked by the compiler.
    for it in item.items.iter_mut() {
        let ImplItem::Fn(method) = it else { continue };
        if !has_attr(&method.attrs, "mutation") {
            continue;
        }
        if has_attr(&method.attrs, "authorize") || has_attr(&method.attrs, "public") {
            continue;
        }
        let Some((subject, subject_ty, entity, action)) = authorized_subject(&method.sig) else {
            continue;
        };
        bind_subject_from_signature(method, service, &subject, &subject_ty, &entity, &action);
    }

    generated.append(&mut item.items);
    item.items = generated;

    Ok(quote! {
        #[::nest_rs_graphql::resolver]
        #item
    })
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().is_ident(name))
}

/// For a parameter typed `Authorized<E, A>`, the `(ident, full type, E, A)`. The
/// full type is reused verbatim in the synthesised `let` so the developer's own
/// import and spelling drive it; `E`/`A` feed the `#[authorize(A, E)]` posture.
fn authorized_subject(sig: &Signature) -> Option<(Ident, Type, Type, Type)> {
    sig.inputs.iter().find_map(|arg| {
        let FnArg::Typed(pt) = arg else { return None };
        let entity = nth_generic_type(&pt.ty, "Authorized", 0)?.clone();
        let action = nth_generic_type(&pt.ty, "Authorized", 1)?.clone();
        let Pat::Ident(pi) = &*pt.pat else { return None };
        Some((pi.ident.clone(), (*pt.ty).clone(), entity, action))
    })
}

/// Rewrite a signature-only mutation in place: declare the `#[authorize(A, E)]`
/// posture, swap the `Authorized<E, A>` parameter for an `id: String` argument,
/// and bind the subject from the resolver's own service field at the body's head.
fn bind_subject_from_signature(
    method: &mut syn::ImplItemFn,
    service: &Ident,
    subject: &Ident,
    subject_ty: &Type,
    entity: &Type,
    action: &Type,
) {
    // Reuse an existing `&Context`, else add one right after `&self` (where
    // async-graphql expects it on the wrapper `#[resolver]` generates).
    let ctx_ident = crate::resolver::ctx_param_ident(&method.sig).unwrap_or_else(|| {
        let ident = format_ident!("__bind_ctx");
        method.sig.inputs.insert(
            1,
            parse_quote!(#ident: &::nest_rs_graphql::async_graphql::Context<'_>),
        );
        ident
    });

    // The subject parameter (not a GraphQL `InputType`) becomes the by-id
    // argument the SDL exposes — uniform `id`, like every other by-id op.
    for input in method.sig.inputs.iter_mut() {
        if let FnArg::Typed(pt) = input
            && matches!(&*pt.pat, Pat::Ident(pi) if pi.ident == *subject)
        {
            *input = parse_quote!(id: ::std::string::String);
        }
    }

    method
        .attrs
        .push(parse_quote!(#[authorize(#action, #entity)]));

    let bind: Stmt = parse_quote! {
        let #subject: #subject_ty =
            ::nest_rs_seaorm::graphql::bind_required_with(&*self.#service, #ctx_ident, &id).await?;
    };
    method.block.stmts.insert(0, bind);
}
