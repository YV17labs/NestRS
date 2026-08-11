//! `#[crud]` — generate standard REST operations on a `#[controller]` impl
//! block (all five by default; a subset with `ops = [list, get, ...]`) and
//! re-emit under `#[routes]`. Read ops delegate to the entity's
//! [`CrudService`] (`access` for by-id route-model binding); the write ops
//! delegate to its opt-in `Creatable`/`Updatable`/`Deletable` impls. A
//! hand-written method overrides its generated counterpart.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ImplItem, ItemImpl, parse_quote};

use nest_rs_codegen::{Paginate, UUID_V7_REQUIRED, impl_self_ident, parse_crud_args};

pub(crate) fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    // `#[crud]` is the generated spelling of the impl half, so it answers a wrong
    // shape exactly as `#[routes]` does — through the edge's one pair constant.
    let item = match crate::controller::HTTP_PAIR.parse_operations(input.into()) {
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
    // Kept for the diagnostic it raises on a non-path `impl` target; the name
    // itself is no longer needed now that the error mapper is one shared fn.
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
    let tag = output
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| "Resource".to_owned());

    // Reject non-UUID-v7 ids before loading — validation half of route-model
    // binding. The wording is shared with the GraphQL `#[crud]` so one edge rule
    // reads identically whichever transport refused it.
    let id_v7_check: TokenStream2 = quote! {
        if __id.0.get_version_num() != 7 {
            return ::core::result::Result::Err(::nest_rs_http::poem::Error::from_string(
                #UUID_V7_REQUIRED,
                ::nest_rs_http::poem::http::StatusCode::BAD_REQUEST,
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
                ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::web::Json<::std::vec::Vec<#output>>> {
                    let __rows = ::nest_rs_seaorm::CrudService::list(&*self.#service)
                        .await
                        .map_err(::nest_rs_seaorm::crud_error)?;
                    ::core::result::Result::Ok(::nest_rs_http::poem::web::Json(
                        __rows.iter().map(#output::from).collect(),
                    ))
                }
            },
            // Keyset pagination (the default): next cursor in `x-next-cursor`
            // so the body stays a plain (maskable) array.
            Paginate::Cursor => parse_quote! {
                #[get("/")]
                // The handler returns a hand-built `Response` (it carries
                // `x-next-cursor`), so the document cannot read the payload off
                // the signature — it is declared instead. Without it the list
                // route advertised no schema at all and a generated client
                // typed the collection as `any`.
                #[api(summary = #summary, tags(#tag), response = ::std::vec::Vec<#output>)]
                async fn list(
                    &self,
                    _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Read, #entity>,
                    __page: ::nest_rs_http::poem::web::Query<::nest_rs_seaorm::PageParams>,
                ) -> ::nest_rs_http::poem::Result<::nest_rs_http::poem::Response> {
                    let __p = ::nest_rs_seaorm::CrudService::page(
                        &*self.#service,
                        __page.0.limit(),
                        __page.0.after_uuid(),
                    )
                    .await
                    .map_err(::nest_rs_seaorm::crud_error)?;
                    let __items: ::std::vec::Vec<#output> =
                        __p.items.iter().map(#output::from).collect();
                    let mut __resp = ::nest_rs_http::poem::IntoResponse::into_response(::nest_rs_http::poem::web::Json(__items));
                    // Infallible by construction (a UUID renders ASCII), but
                    // never `expect` on the per-request path: a failure just
                    // omits the pagination header.
                    if let ::core::option::Option::Some(__cursor) = __p.next_cursor
                        && let ::core::result::Result::Ok(__value) =
                            ::nest_rs_http::poem::http::HeaderValue::from_str(
                                &::std::string::ToString::to_string(&__cursor),
                            )
                    {
                        __resp.headers_mut().insert(
                            ::nest_rs_http::poem::http::HeaderName::from_static("x-next-cursor"),
                            __value,
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
                __id: ::nest_rs_http::poem::web::Path<::nest_rs_resource::uuid::Uuid>,
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
        let summary = format!("Create {tag}");
        generated.push(parse_quote! {
            #[post("/")]
            // The handler hands back a built `Response` (it carries
            // `Location`), so the document cannot read the payload off the
            // signature — it is declared, exactly as the paginated list op
            // declares its array.
            #[api(summary = #summary, tags(#tag), response = #output)]
            #[crud_write]
            // Marker read by `#[routes]`: the handler below builds a `Location`,
            // so the document declares it. Shipping the header without declaring
            // it is the `Retry-After` gap on the throttler's `429`, mirrored.
            #[crud_location]
            // `201 Created` is what a route that mints a resource answers.
            // Declared via `#[http_code]` (not a returned `StatusCode`) so
            // `#[routes]` records it and the OpenAPI document advertises `201`,
            // the same way the delete op advertises its `204` (OAPI-O3).
            #[http_code(201)]
            async fn create(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Create, #entity>,
                // The collection URI as the caller sent it — global prefix and
                // version segment included. Reconstructing it from the mount
                // metadata would re-derive what the request already states.
                // Read through `caller_path` below: the router strips a global
                // prefix off `uri()`, and `original_uri()` is populated on the
                // hyper path only.
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
                // RFC 9110 §15.3.2: a `201` names what it created. Absent only
                // for an entity that does not key on a `Uuid` — every other
                // `#[crud]` route already takes one as its path id.
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
        let summary = format!("Update {tag} by id");
        generated.push(parse_quote! {
            #[patch("/:id")]
            #[api(summary = #summary, tags(#tag))]
            #[crud_write]
            async fn update(
                &self,
                _authz: ::nest_rs_authz::http::Authorize<::nest_rs_authz::Update, #entity>,
                __id: ::nest_rs_http::poem::web::Path<::nest_rs_resource::uuid::Uuid>,
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
                __id: ::nest_rs_http::poem::web::Path<::nest_rs_resource::uuid::Uuid>,
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

    /// R10: the create route builds a `Location`, and `#[routes]` only declares
    /// it in the OpenAPI document when the handler says so — the marker is the
    /// whole seam between "the header ships" and "a generated client can read
    /// it". Asserted on the expansion, because a missing marker is not a
    /// compile error anywhere: it is a silently poorer document.
    #[test]
    fn the_create_op_marks_the_location_it_sends() {
        let out = generated_methods(quote! {
            service = svc, entity = E, output = Thing, create = CreateThing
        });
        assert!(
            out.contains("crud_location"),
            "create stamps the marker `#[routes]` reads: {out}",
        );
        // Read ops send no `Location`, so they must not claim one.
        let read_only = generated_methods(quote! {
            service = svc, entity = E, output = Thing, ops = [list, get]
        });
        assert!(
            !read_only.contains("crud_location"),
            "only the create op declares a Location: {read_only}",
        );
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
