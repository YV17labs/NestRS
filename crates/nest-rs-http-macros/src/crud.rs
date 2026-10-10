//! `#[crud]` — generate standard REST operations on a `#[controller]` impl
//! block and re-emit it under `#[routes]`.

use nest_rs_codegen::pair;
use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ImplItem, ItemImpl, parse_quote};

use nest_rs_codegen::{Paginate, impl_self_ident, parse_crud_args};

pub(crate) fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = match pair::HTTP.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    match crud(args.into(), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

pub(crate) fn crud(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let cfg = parse_crud_args(args)?;
    let ops = cfg.generated_ops()?;
    let self_ty = item.self_ty.clone();
    // Kept for the diagnostic it raises on a non-path `impl` target.
    let _ = impl_self_ident(&self_ty, "#[crud]")?;

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
    let noun = output
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| "Resource".to_owned());

    // Emitted as a path, not interpolated: `Bind<S, A>` and the GraphQL `bind`
    // helper enforce the same rule and must read the same sentence.
    let id_v7_check: TokenStream2 = quote! {
        if __id.0.get_version_num() != 7 {
            return ::core::result::Result::Err(::nest_rs_http::poem::Error::from_string(
                ::nest_rs_core::UUID_V7_REQUIRED,
                ::nest_rs_http::poem::http::StatusCode::BAD_REQUEST,
            ));
        }
    };

    let mut generated: Vec<ImplItem> = Vec::new();

    if ops.list && !existing.contains("list") {
        let summary = format!("List {noun}");
        let list_method: ImplItem = match cfg.paginate {
            Paginate::None => parse_quote! {
                #[get("/")]
                #[api(summary = #summary)]
                async fn list(
                    &self,
                    _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::web::Json<::std::vec::Vec<#output>>> {
                    let __rows = ::nest_rs_seaorm::CrudService::list(&*self.#service)
                        .await
                        .map_err(::nest_rs_seaorm::crud_error)?;
                    ::core::result::Result::Ok(::nest_rs_http::poem::web::Json(
                        __rows.iter().map(#output::from).collect(),
                    ))
                }
            },
            Paginate::Cursor => parse_quote! {
                #[get("/")]
                // A built `Response` hides the payload from the signature: declared here.
                #[api(summary = #summary, response = ::std::vec::Vec<#output>)]
                // Read by `#[routes]` to declare the `Link` in the document.
                #[crud_next_link]
                async fn list(
                    &self,
                    _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                    // Read through `caller_path`: the router strips a global prefix off `uri()`.
                    __req: &::nest_rs_http::poem::Request,
                    __page: ::nest_rs_http::poem::web::Query<::nest_rs_seaorm::PageParams>,
                ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::Response> {
                    let __first = __page.0.limit();
                    let __p = ::nest_rs_seaorm::CrudService::page(
                        &*self.#service,
                        __first,
                        __page.0.after_uuid(),
                    )
                    .await
                    .map_err(::nest_rs_seaorm::crud_error)?;
                    let __items: ::std::vec::Vec<#output> =
                        __p.items.iter().map(#output::from).collect();
                    let mut __resp = ::nest_rs_http::poem::IntoResponse::into_response(::nest_rs_http::poem::web::Json(__items));
                    if let ::core::option::Option::Some(__cursor) = __p.next_cursor {
                        ::nest_rs_http::set_next_link(
                            &mut __resp,
                            ::nest_rs_http::caller_path(__req),
                            __first,
                            __cursor,
                        );
                    }
                    ::core::result::Result::Ok(__resp)
                }
            },
        };
        generated.push(list_method);
    }

    if ops.get && !existing.contains("get") {
        let summary = format!("Fetch {noun} by id");
        generated.push(parse_quote! {
            #[get("/:id")]
            #[api(summary = #summary)]
            async fn get(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                __id: ::nest_rs_http::poem::web::Path<::nest_rs_resource::__private::uuid::Uuid>,
            ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::web::Json<#output>> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Read,
                    __id.0,
                )
                .await
                .map_err(::nest_rs_seaorm::crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::core::result::Result::Ok(::nest_rs_http::poem::web::Json(#output::from(&__m)))
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::nest_rs_http::poem::Error::from_status(::nest_rs_http::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::nest_rs_http::poem::Error::from_status(::nest_rs_http::poem::http::StatusCode::NOT_FOUND),
                    ),
                }
            }
        });
    }

    if let Some(create) = ops.create
        && !existing.contains("create")
    {
        let summary = format!("Create {noun}");
        generated.push(parse_quote! {
            #[post("/")]
            // A built `Response` hides the payload from the signature: declared here.
            #[api(summary = #summary, response = #output)]
            #[crud_write]
            // Read by `#[routes]` to declare the `Location` header in the document.
            #[crud_location]
            // Via `#[http_code]`, not a returned status, so the document advertises it.
            #[http_code(201)]
            async fn create(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Create, #entity>,
                // Read through `caller_path`: the router strips a global prefix off
                // `uri()`, and `original_uri()` is populated on the hyper path only.
                __req: &::nest_rs_http::poem::Request,
                __body: ::nest_rs_http::Valid<::nest_rs_http::poem::web::Json<#create>>,
            ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::Response> {
                let __row = ::nest_rs_seaorm::Creatable::create(
                    &*self.#service,
                    __body.into_inner(),
                )
                .await
                .map_err(::nest_rs_seaorm::crud_error)?;
                let mut __resp = ::nest_rs_http::poem::IntoResponse::into_response(
                    ::nest_rs_http::poem::web::Json(#output::from(&__row)),
                );
                // RFC 9110 §15.3.2: a `201` names what it created.
                if let ::core::option::Option::Some(__id) =
                    ::nest_rs_seaorm::model_uuid::<#entity>(&__row)
                {
                    ::nest_rs_http::set_created_location(
                        &mut __resp,
                        ::nest_rs_http::caller_path(__req),
                        __id,
                    );
                }
                ::core::result::Result::Ok(__resp)
            }
        });
    }

    if let Some(update) = ops.update
        && !existing.contains("update")
    {
        let summary = format!("Update {noun} by id");
        generated.push(parse_quote! {
            #[patch("/:id")]
            #[api(summary = #summary)]
            #[crud_write]
            async fn update(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Update, #entity>,
                __id: ::nest_rs_http::poem::web::Path<::nest_rs_resource::__private::uuid::Uuid>,
                __body: ::nest_rs_http::Valid<::nest_rs_http::poem::web::Json<#update>>,
            ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::web::Json<#output>> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Update,
                    __id.0,
                )
                .await
                .map_err(::nest_rs_seaorm::crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        let __row = ::nest_rs_seaorm::Updatable::update(
                            &*self.#service,
                            __m,
                            __body.into_inner(),
                        )
                        .await
                        .map_err(::nest_rs_seaorm::crud_error)?;
                        ::core::result::Result::Ok(::nest_rs_http::poem::web::Json(#output::from(&__row)))
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::nest_rs_http::poem::Error::from_status(::nest_rs_http::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::nest_rs_http::poem::Error::from_status(::nest_rs_http::poem::http::StatusCode::NOT_FOUND),
                    ),
                }
            }
        });
    }

    if ops.delete && !existing.contains("delete") {
        let summary = format!("Delete {noun} by id");
        generated.push(parse_quote! {
            #[delete("/:id")]
            #[api(summary = #summary)]
            #[crud_write]
            // Via `#[http_code]`, not a returned status, so the document advertises it.
            #[http_code(204)]
            async fn delete(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Delete, #entity>,
                __id: ::nest_rs_http::poem::web::Path<::nest_rs_resource::__private::uuid::Uuid>,
            ) -> ::nest_rs_http::poem::Result<()> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Delete,
                    __id.0,
                )
                .await
                .map_err(::nest_rs_seaorm::crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::nest_rs_seaorm::Deletable::delete(&*self.#service, __m)
                            .await
                            .map_err(::nest_rs_seaorm::crud_error)?;
                        ::core::result::Result::Ok(())
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::nest_rs_http::poem::Error::from_status(::nest_rs_http::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::nest_rs_http::poem::Error::from_status(::nest_rs_http::poem::http::StatusCode::NOT_FOUND),
                    ),
                }
            }
        });
    }

    generated.append(&mut item.items);
    item.items = generated;

    Ok(quote! {
        #[::nest_rs_http::routes]
        #item
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    fn generated_methods(args: TokenStream2) -> String {
        let item: ItemImpl = parse_quote! { impl Things {} };
        crud(args, item).expect("crud generates").to_string()
    }

    #[test]
    fn partial_ops_generate_only_the_listed_routes() {
        let out = generated_methods(quote! {
            service = svc, entity = E, output = Thing, ops = [list, get, delete]
        });
        assert!(out.contains("fn list"), "list expected: {out}");
        assert!(out.contains("fn get"), "get expected: {out}");
        assert!(out.contains("fn delete"), "delete expected: {out}");
        assert!(!out.contains("fn create"), "create must be absent: {out}");
        assert!(!out.contains("fn update"), "update must be absent: {out}");
    }

    // A missing marker is no compile error anywhere, only a poorer document.
    #[test]
    fn the_create_op_marks_the_location_it_sends() {
        let out = generated_methods(quote! {
            service = svc, entity = E, output = Thing, create = CreateThing
        });
        assert!(
            out.contains("crud_location"),
            "create stamps the marker `#[routes]` reads: {out}",
        );
        let read_only = generated_methods(quote! {
            service = svc, entity = E, output = Thing, ops = [list, get]
        });
        assert!(
            !read_only.contains("crud_location"),
            "only the create op declares a Location: {read_only}",
        );
    }

    #[test]
    fn only_the_paginated_list_marks_the_link_it_sends() {
        let paged = generated_methods(quote! {
            service = svc, entity = E, output = Thing, ops = [list]
        });
        assert!(
            paged.contains("crud_next_link"),
            "the cursor list stamps the marker `#[routes]` reads: {paged}",
        );
        let whole = generated_methods(quote! {
            service = svc, entity = E, output = Thing, ops = [list], paginate = none
        });
        assert!(
            !whole.contains("crud_next_link"),
            "a full collection sends no cursor: {whole}",
        );
    }

    #[test]
    fn create_op_without_input_type_fails_to_expand() {
        let item: ItemImpl = parse_quote! { impl Things {} };
        let err = crud(
            quote! { service = svc, entity = E, output = Thing, ops = [create] },
            item,
        )
        .expect_err("create without an input type must fail");
        assert!(err.to_string().contains("create"));
    }
}
