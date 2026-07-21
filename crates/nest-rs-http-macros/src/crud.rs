//! `#[crud]` — generate standard REST operations on a `#[controller]` impl
//! block (all five by default; a subset with `ops = [list, get, ...]`) and
//! re-emit under `#[routes]`. Read ops delegate to the entity's
//! [`CrudService`] (`access` for by-id route-model binding); the write ops
//! delegate to its opt-in `Creatable`/`Updatable`/`Deletable` impls. A
//! hand-written method overrides its generated counterpart.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{ImplItem, ItemImpl, parse_macro_input, parse_quote};

use nest_rs_codegen::{Paginate, impl_self_ident, parse_crud_args};

pub(crate) fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    match crud(args.into(), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

pub(crate) fn crud(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let cfg = parse_crud_args(args)?;
    let ops = cfg.generated_ops()?;
    let self_ty = item.self_ty.clone();
    let base = impl_self_ident(&self_ty, "#[crud]")?;

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
    let tag = output
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| "Resource".to_owned());

    // Per-controller name avoids collisions between two controllers in one module.
    let internal = format_ident!("__nestrs_crud_internal_{}", base);

    // Reject non-UUID-v7 ids before loading — validation half of route-model binding.
    let id_v7_check: TokenStream2 = quote! {
        if __id.0.get_version_num() != 7 {
            return ::core::result::Result::Err(::poem::Error::from_string(
                "path id must be a UUID v7",
                ::poem::http::StatusCode::BAD_REQUEST,
            ));
        }
    };

    let mut generated: Vec<ImplItem> = Vec::new();

    if ops.list && !existing.contains("list") {
        let summary = format!("List {tag}");
        let list_method: ImplItem = match cfg.paginate {
            // Explicit opt-out (`paginate = none`): the full ability-scoped
            // collection, still backstopped by `CrudService::list`'s hard cap.
            Paginate::None => parse_quote! {
                #[get("/")]
                #[api(summary = #summary, tags(#tag))]
                async fn list(
                    &self,
                    _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                ) -> ::poem::Result<::poem::web::Json<::std::vec::Vec<#output>>> {
                    let __rows = ::nest_rs_seaorm::CrudService::list(&*self.#service)
                        .await
                        .map_err(#internal)?;
                    ::core::result::Result::Ok(::poem::web::Json(
                        __rows.iter().map(#output::from).collect(),
                    ))
                }
            },
            // Keyset pagination (the default): next cursor in `x-next-cursor`
            // so the body stays a plain (maskable) array.
            Paginate::Cursor => parse_quote! {
                #[get("/")]
                #[api(summary = #summary, tags(#tag))]
                async fn list(
                    &self,
                    _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                    __page: ::poem::web::Query<::nest_rs_seaorm::PageParams>,
                ) -> ::poem::Result<::poem::Response> {
                    let __p = ::nest_rs_seaorm::CrudService::page(
                        &*self.#service,
                        __page.0.limit(),
                        __page.0.after_uuid(),
                    )
                    .await
                    .map_err(#internal)?;
                    let __items: ::std::vec::Vec<#output> =
                        __p.items.iter().map(#output::from).collect();
                    let mut __resp = ::poem::IntoResponse::into_response(::poem::web::Json(__items));
                    if let ::core::option::Option::Some(__cursor) = __p.next_cursor {
                        __resp.headers_mut().insert(
                            ::poem::http::HeaderName::from_static("x-next-cursor"),
                            ::poem::http::HeaderValue::from_str(
                                &::std::string::ToString::to_string(&__cursor),
                            )
                            .expect("a UUID renders as a valid header value"),
                        );
                    }
                    ::core::result::Result::Ok(__resp)
                }
            },
        };
        generated.push(list_method);
    }

    if ops.get && !existing.contains("get") {
        let summary = format!("Fetch {tag} by id");
        generated.push(parse_quote! {
            #[get("/:id")]
            #[api(summary = #summary, tags(#tag))]
            async fn get(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                __id: ::poem::web::Path<::uuid::Uuid>,
            ) -> ::poem::Result<::poem::web::Json<#output>> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Read,
                    __id.0,
                )
                .await
                .map_err(#internal)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::core::result::Result::Ok(::poem::web::Json(#output::from(&__m)))
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::NOT_FOUND),
                    ),
                }
            }
        });
    }

    if let Some(create) = ops.create
        && !existing.contains("create")
    {
        let summary = format!("Create {tag}");
        generated.push(parse_quote! {
            #[post("/")]
            #[api(summary = #summary, tags(#tag))]
            #[crud_write]
            async fn create(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Create, #entity>,
                __body: ::nest_rs_http::Valid<::poem::web::Json<#create>>,
            ) -> ::poem::Result<::poem::web::Json<#output>> {
                let __row = ::nest_rs_seaorm::Creatable::create(
                    &*self.#service,
                    __body.into_inner(),
                )
                .await
                .map_err(#internal)?;
                ::core::result::Result::Ok(::poem::web::Json(#output::from(&__row)))
            }
        });
    }

    if let Some(update) = ops.update
        && !existing.contains("update")
    {
        let summary = format!("Update {tag} by id");
        generated.push(parse_quote! {
            #[patch("/:id")]
            #[api(summary = #summary, tags(#tag))]
            #[crud_write]
            async fn update(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Update, #entity>,
                __id: ::poem::web::Path<::uuid::Uuid>,
                __body: ::nest_rs_http::Valid<::poem::web::Json<#update>>,
            ) -> ::poem::Result<::poem::web::Json<#output>> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Update,
                    __id.0,
                )
                .await
                .map_err(#internal)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        let __row = ::nest_rs_seaorm::Updatable::update(
                            &*self.#service,
                            __m,
                            __body.into_inner(),
                        )
                        .await
                        .map_err(#internal)?;
                        ::core::result::Result::Ok(::poem::web::Json(#output::from(&__row)))
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::NOT_FOUND),
                    ),
                }
            }
        });
    }

    if ops.delete && !existing.contains("delete") {
        let summary = format!("Delete {tag} by id");
        generated.push(parse_quote! {
            #[delete("/:id")]
            #[api(summary = #summary, tags(#tag))]
            #[crud_write]
            // A successful delete is `204 No Content`. Declared via `#[http_code]`
            // (not a returned `StatusCode`) so `#[routes]` records it on the route
            // and the OpenAPI document advertises `204`, not `200` (OAPI-O3).
            #[http_code(204)]
            async fn delete(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Delete, #entity>,
                __id: ::poem::web::Path<::uuid::Uuid>,
            ) -> ::poem::Result<()> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nest_rs_authz::Action::Delete,
                    __id.0,
                )
                .await
                .map_err(#internal)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::nest_rs_seaorm::Deletable::delete(&*self.#service, __m)
                            .await
                            .map_err(#internal)?;
                        ::core::result::Result::Ok(())
                    }
                    ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::NOT_FOUND),
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

        // Map a write failure to the HTTP status it deserves instead of a
        // blanket 500: a unique-constraint violation is a 409, a create the
        // ability re-check rolled back (`RecordNotInserted`) is a 403, a row
        // that vanished between the access check and the write is a 404. Only a
        // genuinely unexpected `DbErr` is a 500 — and it ships an empty body, so
        // the raw driver message never reaches the client.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #internal(__e: ::sea_orm::DbErr) -> ::poem::Error {
            let __status = match ::sea_orm::DbErr::sql_err(&__e) {
                ::core::option::Option::Some(
                    ::sea_orm::SqlErr::UniqueConstraintViolation(_),
                ) => ::poem::http::StatusCode::CONFLICT,
                _ => match __e {
                    ::sea_orm::DbErr::RecordNotInserted => {
                        ::poem::http::StatusCode::FORBIDDEN
                    }
                    ::sea_orm::DbErr::RecordNotUpdated
                    | ::sea_orm::DbErr::RecordNotFound(_) => {
                        ::poem::http::StatusCode::NOT_FOUND
                    }
                    _ => ::poem::http::StatusCode::INTERNAL_SERVER_ERROR,
                },
            };
            ::poem::Error::from_status(__status)
        }
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

    // `ops = [list, get, delete]` generates exactly those three routes — no
    // `create`/`update`, and no need for `create = `/`update = ` input types.
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

    // Requesting a write op without its input type is a hard macro error.
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
