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
use syn::{ImplItem, ItemImpl, parse_macro_input, parse_quote};

use nest_rs_codegen::{Paginate, parse_crud_args, singular_of};

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
    // Global validation: create/update inputs run `validator::Validate` before
    // the service write — the NestJS `ValidationPipe` analog. `ValidateProbe`
    // no-ops when the input type carries no rules (compile-time dispatch), so
    // this is free for inputs without `#[validate]`. Runs before the DB load so
    // a malformed input never reaches the service.
    let validate_input: TokenStream2 = quote! {
        {
            use ::nest_rs_graphql::MaybeValidateFallback as _;
            ::nest_rs_graphql::ValidateProbe(&input)
                .maybe_validate()
                .map_err(#gql_err)?;
        }
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
                #validate_input
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
                #validate_input
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

    generated.append(&mut item.items);
    item.items = generated;

    Ok(quote! {
        #[::nest_rs_graphql::resolver]
        #item
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    // A `create` op listed without its `create = <Input>` type is a hard macro
    // error — a write op is never silently dropped or exposed as a no-op
    // mutation on the wire. (GraphQL analog of the same gate the HTTP `#[crud]`
    // already tests.)
    #[test]
    fn create_op_without_input_type_fails_to_expand() {
        let item: ItemImpl = parse_quote! { impl Things {} };
        let err = crud(
            quote! { service = svc, entity = E, output = Thing, ops = [create] },
            item,
        )
        .expect_err("create without an input type must fail to expand");
        assert!(err.to_string().contains("create"), "names the op: {}", err);
    }

    // The same gate for `update`.
    #[test]
    fn update_op_without_input_type_fails_to_expand() {
        let item: ItemImpl = parse_quote! { impl Things {} };
        let err = crud(
            quote! { service = svc, entity = E, output = Thing, ops = [update] },
            item,
        )
        .expect_err("update without an input type must fail to expand");
        assert!(err.to_string().contains("update"), "names the op: {}", err);
    }

    // The valid form — a write op paired with its input type — expands and
    // emits the operation.
    #[test]
    fn create_op_with_input_type_expands() {
        let item: ItemImpl = parse_quote! { impl Things {} };
        let out = crud(
            quote! {
                service = svc, entity = E, output = Thing, ops = [create], create = CreateThing
            },
            item,
        )
        .expect("create paired with its input type expands")
        .to_string();
        assert!(out.contains("create_thing"), "emits the create op: {out}");
    }
}
