//! `#[crud]` — generate standard REST operations on a `#[controller]` impl
//! block (`list` + `get` always; `create`/`update`/`delete` unless `readonly`)
//! and re-emit under `#[routes]`. Handlers delegate to the entity's
//! [`CrudService`] (`access` for by-id route-model binding); a hand-written
//! method overrides its generated counterpart.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{ImplItem, ItemImpl, parse_macro_input, parse_quote};

use nestrs_codegen::{Paginate, impl_self_ident, parse_crud_args};

pub(crate) fn entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    match crud(args.into(), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

pub(crate) fn crud(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let cfg = parse_crud_args(args)?;
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

    if !existing.contains("list") {
        let summary = format!("List {tag}");
        let list_method: ImplItem = match cfg.paginate {
            None => parse_quote! {
                #[get("/")]
                #[api(summary = #summary, tags(#tag))]
                async fn list(
                    &self,
                    _authz: ::nestrs_authz::http::Authorize<::nestrs_authz::Read, #entity>,
                ) -> ::poem::Result<::poem::web::Json<::std::vec::Vec<#output>>> {
                    let __rows = ::nestrs_seaorm::CrudService::list(&*self.#service)
                        .await
                        .map_err(#internal)?;
                    ::core::result::Result::Ok(::poem::web::Json(
                        __rows.iter().map(#output::from).collect(),
                    ))
                }
            },
            // Keyset pagination: next cursor in `x-next-cursor` so the body
            // stays a plain (maskable) array.
            Some(Paginate::Cursor) => parse_quote! {
                #[get("/")]
                #[api(summary = #summary, tags(#tag))]
                async fn list(
                    &self,
                    _authz: ::nestrs_authz::http::Authorize<::nestrs_authz::Read, #entity>,
                    __page: ::poem::web::Query<::nestrs_seaorm::PageParams>,
                ) -> ::poem::Result<::poem::Response> {
                    let __p = ::nestrs_seaorm::CrudService::page(
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
            Some(Paginate::Page) => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "#[crud] REST list does not yet support `paginate = page` (offset); \
                     use `paginate = cursor`",
                ));
            }
        };
        generated.push(list_method);
    }

    if !existing.contains("get") {
        let summary = format!("Fetch {tag} by id");
        generated.push(parse_quote! {
            #[get("/:id")]
            #[api(summary = #summary, tags(#tag))]
            async fn get(
                &self,
                _authz: ::nestrs_authz::http::Authorize<::nestrs_authz::Read, #entity>,
                __id: ::poem::web::Path<::uuid::Uuid>,
            ) -> ::poem::Result<::poem::web::Json<#output>> {
                #id_v7_check
                match ::nestrs_seaorm::CrudService::access(
                    &*self.#service,
                    ::nestrs_authz::Action::Read,
                    __id.0,
                )
                .await
                .map_err(#internal)?
                {
                    ::nestrs_seaorm::Access::Found(__m) => {
                        ::core::result::Result::Ok(::poem::web::Json(#output::from(&__m)))
                    }
                    ::nestrs_seaorm::Access::Denied => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::FORBIDDEN),
                    ),
                    ::nestrs_seaorm::Access::Missing => ::core::result::Result::Err(
                        ::poem::Error::from_status(::poem::http::StatusCode::NOT_FOUND),
                    ),
                }
            }
        });
    }

    if !cfg.readonly {
        if let Some(create) = &cfg.create
            && !existing.contains("create")
        {
            let summary = format!("Create {tag}");
            generated.push(parse_quote! {
                #[post("/")]
                #[api(summary = #summary, tags(#tag))]
                async fn create(
                    &self,
                    _authz: ::nestrs_authz::http::Authorize<::nestrs_authz::Create, #entity>,
                    __body: ::nestrs_http::Valid<::poem::web::Json<#create>>,
                ) -> ::poem::Result<::poem::web::Json<#output>> {
                    let __row = ::nestrs_seaorm::CrudService::create(
                        &*self.#service,
                        __body.into_inner(),
                    )
                    .await
                    .map_err(#internal)?;
                    ::core::result::Result::Ok(::poem::web::Json(#output::from(&__row)))
                }
            });
        }

        if let Some(update) = &cfg.update
            && !existing.contains("update")
        {
            let summary = format!("Update {tag} by id");
            generated.push(parse_quote! {
                #[patch("/:id")]
                #[api(summary = #summary, tags(#tag))]
                async fn update(
                    &self,
                    _authz: ::nestrs_authz::http::Authorize<::nestrs_authz::Update, #entity>,
                    __id: ::poem::web::Path<::uuid::Uuid>,
                    __body: ::nestrs_http::Valid<::poem::web::Json<#update>>,
                ) -> ::poem::Result<::poem::web::Json<#output>> {
                    #id_v7_check
                    match ::nestrs_seaorm::CrudService::access(
                        &*self.#service,
                        ::nestrs_authz::Action::Update,
                        __id.0,
                    )
                    .await
                    .map_err(#internal)?
                    {
                        ::nestrs_seaorm::Access::Found(__m) => {
                            let __row = ::nestrs_seaorm::CrudService::update(
                                &*self.#service,
                                __m,
                                __body.into_inner(),
                            )
                            .await
                            .map_err(#internal)?;
                            ::core::result::Result::Ok(::poem::web::Json(#output::from(&__row)))
                        }
                        ::nestrs_seaorm::Access::Denied => ::core::result::Result::Err(
                            ::poem::Error::from_status(::poem::http::StatusCode::FORBIDDEN),
                        ),
                        ::nestrs_seaorm::Access::Missing => ::core::result::Result::Err(
                            ::poem::Error::from_status(::poem::http::StatusCode::NOT_FOUND),
                        ),
                    }
                }
            });
        }

        if !existing.contains("delete") {
            let summary = format!("Delete {tag} by id");
            generated.push(parse_quote! {
                #[delete("/:id")]
                #[api(summary = #summary, tags(#tag))]
                async fn delete(
                    &self,
                    _authz: ::nestrs_authz::http::Authorize<::nestrs_authz::Delete, #entity>,
                    __id: ::poem::web::Path<::uuid::Uuid>,
                ) -> ::poem::Result<::poem::http::StatusCode> {
                    #id_v7_check
                    match ::nestrs_seaorm::CrudService::access(
                        &*self.#service,
                        ::nestrs_authz::Action::Delete,
                        __id.0,
                    )
                    .await
                    .map_err(#internal)?
                    {
                        ::nestrs_seaorm::Access::Found(__m) => {
                            ::nestrs_seaorm::CrudService::delete(&*self.#service, __m)
                                .await
                                .map_err(#internal)?;
                            ::core::result::Result::Ok(::poem::http::StatusCode::NO_CONTENT)
                        }
                        ::nestrs_seaorm::Access::Denied => ::core::result::Result::Err(
                            ::poem::Error::from_status(::poem::http::StatusCode::FORBIDDEN),
                        ),
                        ::nestrs_seaorm::Access::Missing => ::core::result::Result::Err(
                            ::poem::Error::from_status(::poem::http::StatusCode::NOT_FOUND),
                        ),
                    }
                }
            });
        }
    }

    generated.append(&mut item.items);
    item.items = generated;

    Ok(quote! {
        #[::nestrs_http::routes]
        #item

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #internal<E: ::std::string::ToString>(__e: E) -> ::poem::Error {
            ::poem::Error::from_string(
                ::std::string::ToString::to_string(&__e),
                ::poem::http::StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    })
}
