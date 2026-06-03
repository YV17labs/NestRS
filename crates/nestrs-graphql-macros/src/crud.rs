//! `#[crud]` — synthesise the standard resolver operations the developer did
//! not hand-write, then re-emit under `#[resolver]`. Every operation
//! delegates to the entity's [`CrudService`] — never `Repo` directly.
//! Override by writing the matching method.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, parse_quote, ImplItem, ItemImpl};

use nestrs_codegen::{parse_crud_args, singular_of};

pub fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    match crud(TokenStream2::from(args), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn crud(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let cfg = parse_crud_args(args)?;

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
            .map_err(|__e| ::nestrs_graphql::async_graphql::Error::new(
                ::std::string::ToString::to_string(&__e),
            ))?;
        if __id.get_version_num() != 7 {
            return ::core::result::Result::Err(
                ::nestrs_graphql::async_graphql::Error::new("id must be a UUID v7"),
            );
        }
    };
    let gql_err: TokenStream2 = quote! {
        |__e| ::nestrs_graphql::async_graphql::Error::new(::std::string::ToString::to_string(&__e))
    };
    let forbidden: TokenStream2 = quote! {
        ::nestrs_graphql::async_graphql::Error::new("forbidden")
    };

    let mut generated: Vec<ImplItem> = Vec::new();

    if !existing.contains(&list_op.to_string()) {
        generated.push(parse_quote! {
            #[query]
            async fn #list_op(
                &self,
                __ctx: &::nestrs_graphql::async_graphql::Context<'_>,
            ) -> ::nestrs_graphql::async_graphql::Result<::std::vec::Vec<#output>> {
                ::nestrs_authz::graphql::authorize::<::nestrs_authz::Read, #entity>(__ctx)?;
                let __rows = ::nestrs_database::CrudService::list(&*self.#service)
                    .await
                    .map_err(#gql_err)?;
                ::core::result::Result::Ok(__rows.iter().map(#output::from).collect())
            }
        });
    }

    if !existing.contains(&get_op.to_string()) {
        generated.push(parse_quote! {
            #[query]
            async fn #get_op(
                &self,
                __ctx: &::nestrs_graphql::async_graphql::Context<'_>,
                id: ::std::string::String,
            ) -> ::nestrs_graphql::async_graphql::Result<::core::option::Option<#output>> {
                let _ = __ctx;
                #parse_id
                match ::nestrs_database::CrudService::access(
                    &*self.#service,
                    ::nestrs_authz::Action::Read,
                    __id,
                )
                .await
                .map_err(#gql_err)?
                {
                    ::nestrs_database::Access::Found(__m) => {
                        ::core::result::Result::Ok(::core::option::Option::Some(#output::from(&__m)))
                    }
                    ::nestrs_database::Access::Denied => ::core::result::Result::Err(#forbidden),
                    ::nestrs_database::Access::Missing => {
                        ::core::result::Result::Ok(::core::option::Option::None)
                    }
                }
            }
        });
    }

    if !cfg.readonly {
        if let Some(create) = &cfg.create {
            if !existing.contains(&create_op.to_string()) {
                generated.push(parse_quote! {
                    #[mutation]
                    async fn #create_op(
                        &self,
                        __ctx: &::nestrs_graphql::async_graphql::Context<'_>,
                        input: #create,
                    ) -> ::nestrs_graphql::async_graphql::Result<#output> {
                        ::nestrs_authz::graphql::authorize::<::nestrs_authz::Create, #entity>(__ctx)?;
                        let __row = ::nestrs_database::CrudService::create(&*self.#service, input)
                            .await
                            .map_err(#gql_err)?;
                        ::core::result::Result::Ok(#output::from(&__row))
                    }
                });
            }
        }

        if let Some(update) = &cfg.update {
            if !existing.contains(&update_op.to_string()) {
                generated.push(parse_quote! {
                    #[mutation]
                    async fn #update_op(
                        &self,
                        __ctx: &::nestrs_graphql::async_graphql::Context<'_>,
                        id: ::std::string::String,
                        input: #update,
                    ) -> ::nestrs_graphql::async_graphql::Result<::core::option::Option<#output>> {
                        let _ = __ctx;
                        #parse_id
                        match ::nestrs_database::CrudService::access(
                            &*self.#service,
                            ::nestrs_authz::Action::Update,
                            __id,
                        )
                        .await
                        .map_err(#gql_err)?
                        {
                            ::nestrs_database::Access::Found(__m) => {
                                let __row = ::nestrs_database::CrudService::update(
                                    &*self.#service,
                                    __m,
                                    input,
                                )
                                .await
                                .map_err(#gql_err)?;
                                ::core::result::Result::Ok(::core::option::Option::Some(#output::from(&__row)))
                            }
                            ::nestrs_database::Access::Denied => ::core::result::Result::Err(#forbidden),
                            ::nestrs_database::Access::Missing => {
                                ::core::result::Result::Ok(::core::option::Option::None)
                            }
                        }
                    }
                });
            }
        }

        if !existing.contains(&delete_op.to_string()) {
            generated.push(parse_quote! {
                #[mutation]
                async fn #delete_op(
                    &self,
                    __ctx: &::nestrs_graphql::async_graphql::Context<'_>,
                    id: ::std::string::String,
                ) -> ::nestrs_graphql::async_graphql::Result<bool> {
                    let _ = __ctx;
                    #parse_id
                    match ::nestrs_database::CrudService::access(
                        &*self.#service,
                        ::nestrs_authz::Action::Delete,
                        __id,
                    )
                    .await
                    .map_err(#gql_err)?
                    {
                        ::nestrs_database::Access::Found(__m) => {
                            ::nestrs_database::CrudService::delete(&*self.#service, __m)
                                .await
                                .map_err(#gql_err)?;
                            ::core::result::Result::Ok(true)
                        }
                        ::nestrs_database::Access::Denied => ::core::result::Result::Err(#forbidden),
                        ::nestrs_database::Access::Missing => ::core::result::Result::Ok(false),
                    }
                }
            });
        }
    }

    generated.append(&mut item.items);
    item.items = generated;

    Ok(quote! {
        #[::nestrs_graphql::resolver]
        #item
    })
}
