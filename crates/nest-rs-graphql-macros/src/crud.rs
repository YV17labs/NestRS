//! `#[crud]` — synthesise the standard resolver operations the developer did
//! not hand-write, each delegating to the entity's `CrudService` and typed off
//! it, then re-emit under `#[operations]`.

use nest_rs_codegen::__private::pair;
use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{ImplItem, ItemImpl, parse_quote};

use nest_rs_codegen::__private::{
    CrudOp, CrudTypes, Paginate, impl_self_ident, parse_crud_args, resolver_singular,
};

pub(crate) fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = match pair::GRAPHQL.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    match crud(TokenStream2::from(args), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn crud(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let cfg = parse_crud_args(args)?;
    if let Some(written) = cfg.api_written() {
        return Err(syn::Error::new(
            written,
            format!(
                "{} documents the generated routes' OpenAPI operations, and a resolver serves \
                 none — write the operation by hand, with its doc comment as its description",
                nest_rs_codegen::__private::site("crud", Some("api")),
            ),
        ));
    }
    let existing: HashSet<String> = item
        .items
        .iter()
        .filter_map(|it| match it {
            ImplItem::Fn(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect();

    let host = impl_self_ident(&item.self_ty, "#[crud]")?;
    let service = &cfg.service;
    let singular = resolver_singular(&host);
    let ops = cfg.generated_ops(&existing, |op| op.resolver_method(&singular))?;
    let CrudTypes {
        entity,
        output,
        create,
        update,
        aliases,
    } = cfg.types(&host, &item.self_ty, ops);
    let list_op = format_ident!("{}", CrudOp::List.resolver_method(&singular));
    let get_op = format_ident!("{}", CrudOp::Get.resolver_method(&singular));
    let create_op = format_ident!("{}", CrudOp::Create.resolver_method(&singular));
    let update_op = format_ident!("{}", CrudOp::Update.resolver_method(&singular));
    let delete_op = format_ident!("{}", CrudOp::Delete.resolver_method(&singular));

    let parse_id: TokenStream2 = quote! {
        let __id = ::nest_rs_seaorm::graphql::parse_v7(&id)?;
    };
    let crud_error: TokenStream2 = quote! {
        ::nest_rs_seaorm::__private::graphql_crud_error
    };
    let forbidden: TokenStream2 = quote! {
        ::nest_rs_authz::graphql::forbidden()
    };

    let mut generated: Vec<ImplItem> = Vec::new();

    if ops.list {
        let list_method: ImplItem = match cfg.paginate {
            // A plain `Vec`, so the automatic response mask applies unchanged.
            Paginate::Cursor => parse_quote! {
                #[query]
                #[authorize(::nest_rs_authz::Read, #entity)]
                async fn #list_op(
                    &self,
                    first: ::core::option::Option<u64>,
                    after: ::core::option::Option<::std::string::String>,
                ) -> ::nest_rs_graphql::async_graphql::Result<::std::vec::Vec<#output>> {
                    let __after = ::nest_rs_seaorm::__private::graphql_page_cursor(
                        ::core::option::Option::as_deref(&after),
                    )?;
                    let __page = ::nest_rs_seaorm::CrudService::page(
                        &*self.#service,
                        ::core::option::Option::unwrap_or(
                            first,
                            ::nest_rs_seaorm::DEFAULT_PAGE_SIZE,
                        ),
                        __after,
                    )
                    .await
                    .map_err(#crud_error)?;
                    ::core::result::Result::Ok(
                        __page.items.iter().map(<#output>::from).collect(),
                    )
                }
            },
            Paginate::None => parse_quote! {
                #[query]
                #[authorize(::nest_rs_authz::Read, #entity)]
                async fn #list_op(
                    &self,
                ) -> ::nest_rs_graphql::async_graphql::Result<::std::vec::Vec<#output>> {
                    let __rows = ::nest_rs_seaorm::CrudService::list(&*self.#service)
                        .await
                        .map_err(#crud_error)?;
                    ::core::result::Result::Ok(__rows.iter().map(<#output>::from).collect())
                }
            },
        };
        generated.push(list_method);
    }

    if ops.get {
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
                .map_err(#crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => ::core::result::Result::Ok(
                        ::core::option::Option::Some(<#output>::from(&__m)),
                    ),
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(#forbidden),
                    ::nest_rs_seaorm::Access::Missing => {
                        ::core::result::Result::Ok(::core::option::Option::None)
                    }
                }
            }
        });
    }

    if ops.create {
        generated.push(parse_quote! {
            #[mutation]
            #[authorize(::nest_rs_authz::Create, #entity)]
            async fn #create_op(
                &self,
                input: #create,
            ) -> ::nest_rs_graphql::async_graphql::Result<#output> {
                let __row = ::nest_rs_seaorm::Creatable::create(&*self.#service, input)
                    .await
                    .map_err(#crud_error)?;
                ::core::result::Result::Ok(<#output>::from(&__row))
            }
        });
    }

    if ops.update {
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
                .map_err(#crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        let __row = ::nest_rs_seaorm::Updatable::update(
                            &*self.#service,
                            __m,
                            input,
                        )
                        .await
                        .map_err(#crud_error)?;
                        ::core::result::Result::Ok(::core::option::Option::Some(
                            <#output>::from(&__row),
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

    if ops.delete {
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
                .map_err(#crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::nest_rs_seaorm::Deletable::delete(&*self.#service, __m)
                            .await
                            .map_err(#crud_error)?;
                        ::core::result::Result::Ok(true)
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(#forbidden),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Ok(false),
                }
            }
        });
    }

    generated.append(&mut item.items);
    item.items = generated;

    Ok(quote! {
        #aliases

        #[::nest_rs_graphql::operations]
        #item
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    fn generated(args: TokenStream2, item: ItemImpl) -> syn::Result<String> {
        crud(args, item).map(|tokens| tokens.to_string())
    }

    #[test]
    fn the_service_alone_names_every_operation_off_the_resolvers_resource() {
        let item: ItemImpl = parse_quote! { impl ThingsResolver {} };
        let out = generated(quote! { service = svc }, item).expect("expands");
        for op in [
            "fn things",
            "fn thing",
            "fn create_thing",
            "fn update_thing",
            "fn delete_thing",
        ] {
            assert!(out.contains(op), "{op}: {out}");
        }
        assert!(
            out.contains("input : __NestrsCrudThingsResolverCreate"),
            "the create input is the service's: {out}"
        );
        assert!(
            out.contains("Result < __NestrsCrudThingsResolverOutput >"),
            "the answer is the entity's wire type: {out}"
        );
    }

    #[test]
    fn a_host_named_in_the_singular_still_lists_in_the_plural() {
        let item: ItemImpl = parse_quote! { impl ThingResolver {} };
        let out = generated(quote! { service = svc, ops = [list, get] }, item).expect("expands");
        assert!(
            out.contains("fn things") && out.contains("fn thing ("),
            "{out}"
        );
    }

    #[test]
    fn a_paginate_for_an_excluded_list_fails_to_expand() {
        let item: ItemImpl = parse_quote! { impl ThingsResolver {} };
        let err = generated(quote! { service = svc, ops = [get], paginate = none }, item)
            .expect_err("a `paginate` for an excluded `list` must fail to expand");
        assert!(
            err.to_string()
                .contains("`paginate` configures the `list` op"),
            "names the key and the op: {err}",
        );
    }

    #[test]
    fn a_paginate_beside_a_hand_written_list_fails_to_expand() {
        let item: ItemImpl = parse_quote! {
            impl ThingsResolver {
                async fn things(&self) -> Vec<Thing> { Vec::new() }
            }
        };
        let err = generated(quote! { service = svc, paginate = none }, item)
            .expect_err("a `paginate` beside a hand-written list must fail to expand");
        assert_eq!(
            err.to_string(),
            "#[crud] `paginate` configures the `list` op, which this block writes by hand as \
             `fn things` — drop `paginate = …`: the method bounds its own list",
        );
    }

    #[test]
    fn an_op_the_block_writes_is_not_generated_nor_its_input_named() {
        let item: ItemImpl = parse_quote! {
            impl ThingsResolver {
                async fn create_thing(&self) -> bool { true }
            }
        };
        let out = generated(quote! { service = svc }, item).expect("expands");
        assert!(
            !out.contains("type __NestrsCrudThingsResolverCreate"),
            "{out}"
        );
        assert!(out.contains("fn update_thing"), "{out}");
    }
}
