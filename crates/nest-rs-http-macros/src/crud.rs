//! `#[crud]` — generate standard REST operations on a `#[controller]` impl
//! block, typed off the service its key names, and re-emit it under
//! `#[routes]`.

use nest_rs_codegen::__private::pair;
use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ImplItem, ItemImpl, parse_quote};

use nest_rs_codegen::__private::{
    CHECKED_BY_CRUD, CrudDeclaration, CrudOp, CrudTypes, GeneratedOps, Paginate, cfg_attrs,
    impl_self_ident, parse_crud_args, resource_noun, unit_of_work_required,
};

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
    let self_ty = item.self_ty.clone();
    let host = impl_self_ident(&self_ty, "#[crud]")?;

    let existing: HashSet<String> = item
        .items
        .iter()
        .filter_map(|it| match it {
            ImplItem::Fn(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect();
    let ops = cfg.generated_ops(&existing, |op| op.name().to_owned())?;
    let prefix_check = by_id_path_check(&self_ty, ops);

    let CrudTypes {
        service,
        entity,
        output,
        create,
        update,
        aliases,
    } = cfg.types(&host, &self_ty, ops);
    let noun = resource_noun(&host, "Controller");

    // Emitted as a path, not interpolated: the loader `bind = Service` runs on
    // every edge enforces the same rule and must read the same sentence.
    let id_v7_check: TokenStream2 = quote! {
        if __id.0.get_version_num() != 7 {
            return ::core::result::Result::Err(::nest_rs_http::HttpError::from(
                ::nest_rs_http::ProblemDetails::bad_request()
                    .with_detail(::nest_rs_core::UUID_V7_REQUIRED),
            ));
        }
    };

    let access_refused: TokenStream2 = quote! {
        ::nest_rs_seaorm::Access::Denied => ::core::result::Result::Err(
            ::nest_rs_http::HttpError::from_status(::nest_rs_http::StatusCode::FORBIDDEN),
        ),
        ::nest_rs_seaorm::Access::Missing => ::core::result::Result::Err(
            ::nest_rs_http::HttpError::from_status(::nest_rs_http::StatusCode::NOT_FOUND),
        ),
    };

    // The parameters generated here declare neither a gate nor a shaper.
    let unprobed = proc_macro2::Ident::new(crate::routes::UNPROBED, proc_macro2::Span::call_site());

    let mut generated: Vec<ImplItem> = Vec::new();

    if ops.list {
        let api = api_args(&cfg, CrudOp::List, format!("List {noun}"));
        let list_method: ImplItem = match cfg.paginate {
            Paginate::None => parse_quote! {
                #[get("/")]
                #[api(#api)]
                #[authorize(::nest_rs_authz::Read, #entity)]
                async fn list(
                    &self,
                ) -> ::nest_rs_http::Result<::nest_rs_http::Json<::std::vec::Vec<#output>>> {
                    let __rows = ::nest_rs_seaorm::CrudService::list(#service(self))
                        .await
                        .map_err(::nest_rs_seaorm::__private::http_crud_error)?;
                    ::core::result::Result::Ok(::nest_rs_http::Json(
                        __rows.iter().map(<#output>::from).collect(),
                    ))
                }
            },
            Paginate::Cursor => parse_quote! {
                #[get("/")]
                #[api(#api)]
                #[authorize(::nest_rs_authz::Read, #entity)]
                async fn list(
                    &self,
                    #[#unprobed] __page: ::nest_rs_http::Query<::nest_rs_seaorm::PageParams>,
                ) -> ::nest_rs_http::Result<
                    ::nest_rs_http::Paginated<::nest_rs_http::Json<::std::vec::Vec<#output>>>,
                > {
                    let __first = __page.0.limit();
                    let __after = __page
                        .0
                        .cursor()
                        .map_err(|__e| ::nest_rs_http::pipe_error(&__e))?;
                    let __p = ::nest_rs_seaorm::CrudService::page(
                        #service(self),
                        __first,
                        __after,
                    )
                    .await
                    .map_err(::nest_rs_seaorm::__private::http_crud_error)?;
                    let __items: ::std::vec::Vec<#output> =
                        __p.items.iter().map(<#output>::from).collect();
                    ::core::result::Result::Ok(::nest_rs_http::Paginated::new(
                        ::nest_rs_http::Json(__items),
                        __first,
                        __p.next_cursor,
                    ))
                }
            },
        };
        generated.push(list_method);
    }

    if ops.get {
        let api = api_args(&cfg, CrudOp::Get, format!("Fetch {noun} by id"));
        generated.push(parse_quote! {
            #[get("/{id}")]
            #[api(#api)]
            #[authorize(::nest_rs_authz::Read, #entity)]
            async fn get(
                &self,
                #[#unprobed] __id: ::nest_rs_http::Path<::nest_rs_seaorm::__private::uuid::Uuid>,
            ) -> ::nest_rs_http::Result<::nest_rs_http::Json<#output>> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    #service(self),
                    ::nest_rs_authz::Action::Read,
                    __id.0,
                )
                .await
                .map_err(::nest_rs_seaorm::__private::http_crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::core::result::Result::Ok(::nest_rs_http::Json(<#output>::from(&__m)))
                    }
                    #access_refused
                }
            }
        });
    }

    if ops.create {
        let api = api_args(&cfg, CrudOp::Create, format!("Create {noun}"));
        generated.push(parse_quote! {
            #[post("/")]
            #[api(#api)]
            #[crud_write]
            #[authorize(::nest_rs_authz::Create, #entity)]
            async fn create(
                &self,
                #[#unprobed] __body: ::nest_rs_http::Json<#create>,
            ) -> ::nest_rs_http::Result<::nest_rs_http::Created<::nest_rs_http::Json<#output>>> {
                let __row = ::nest_rs_seaorm::Creatable::create(
                    #service(self),
                    __body.into_inner(),
                )
                .await
                .map_err(::nest_rs_seaorm::__private::http_crud_error)?;
                ::core::result::Result::Ok(::nest_rs_http::Created::at(
                    ::nest_rs_seaorm::__private::created_key::<#entity>(&__row),
                    ::nest_rs_http::Json(<#output>::from(&__row)),
                ))
            }
        });
    }

    if ops.update {
        let api = api_args(&cfg, CrudOp::Update, format!("Update {noun} by id"));
        generated.push(parse_quote! {
            // RFC 9110 §9.3.4: `Update<E>` carries every writable field, which
            // replace the row's — a replacement, not RFC 5789's partial patch.
            #[put("/{id}")]
            #[api(#api)]
            #[crud_write]
            #[authorize(::nest_rs_authz::Update, #entity)]
            async fn update(
                &self,
                #[#unprobed] __id: ::nest_rs_http::Path<::nest_rs_seaorm::__private::uuid::Uuid>,
                #[#unprobed] __body: ::nest_rs_http::Json<#update>,
            ) -> ::nest_rs_http::Result<::nest_rs_http::Json<#output>> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    #service(self),
                    ::nest_rs_authz::Action::Update,
                    __id.0,
                )
                .await
                .map_err(::nest_rs_seaorm::__private::http_crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        let __row = ::nest_rs_seaorm::Updatable::update(
                            #service(self),
                            __m,
                            __body.into_inner(),
                        )
                        .await
                        .map_err(::nest_rs_seaorm::__private::http_crud_error)?;
                        ::core::result::Result::Ok(::nest_rs_http::Json(<#output>::from(&__row)))
                    }
                    #access_refused
                }
            }
        });
    }

    if ops.delete {
        let api = api_args(&cfg, CrudOp::Delete, format!("Delete {noun} by id"));
        generated.push(parse_quote! {
            #[delete("/{id}")]
            #[api(#api)]
            #[crud_write]
            // Via `#[http_code]`, not a returned status, so the document advertises it.
            #[http_code(204)]
            #[authorize(::nest_rs_authz::Delete, #entity)]
            async fn delete(
                &self,
                #[#unprobed] __id: ::nest_rs_http::Path<::nest_rs_seaorm::__private::uuid::Uuid>,
            ) -> ::nest_rs_http::Result<()> {
                #id_v7_check
                match ::nest_rs_seaorm::CrudService::access(
                    #service(self),
                    ::nest_rs_authz::Action::Delete,
                    __id.0,
                )
                .await
                .map_err(::nest_rs_seaorm::__private::http_crud_error)?
                {
                    ::nest_rs_seaorm::Access::Found(__m) => {
                        ::nest_rs_seaorm::Deletable::delete(#service(self), __m)
                            .await
                            .map_err(::nest_rs_seaorm::__private::http_crud_error)?;
                        ::core::result::Result::Ok(())
                    }
                    #access_refused
                }
            }
        });
    }

    generated.append(&mut item.items);
    item.items = generated;
    let unit_of_work = unit_of_work_required(&self_ty, &cfg_attrs(&item.attrs));

    let checked = proc_macro2::Ident::new(CHECKED_BY_CRUD, proc_macro2::Span::call_site());
    Ok(quote! {
        #aliases

        #[::nest_rs_http::routes(#checked)]
        #item

        #prefix_check

        #unit_of_work
    })
}

/// The assertion that the controller's path names no parameter of its own
/// when a generated op reads the id — `#[crud]` never sees that path, and
/// each by-id op reads one parameter, its route's.
fn by_id_path_check(self_ty: &syn::Type, ops: GeneratedOps) -> Option<TokenStream2> {
    let by_id: Vec<String> = [
        (ops.get, CrudOp::Get),
        (ops.update, CrudOp::Update),
        (ops.delete, CrudOp::Delete),
    ]
    .into_iter()
    .filter(|&(generated, _)| generated)
    .map(|(_, op)| format!("`{}`", op.name()))
    .collect();
    if by_id.is_empty() {
        return None;
    }
    let ops = by_id.join(", ");
    let message = format!(
        "#[crud] reads the id of {ops} from the route's one path parameter, and the \
         `#[controller]` path names parameters of its own — mount the resource under a path \
         naming none, or write {ops} by hand",
    );
    Some(quote! {
        const _: () = ::core::assert!(
            <#self_ty>::__nestrs_controller_path_parameters() == 0,
            #message,
        );
    })
}

/// The `summary` and `description` an operation's `#[api]` carries: what
/// `api(<op>(…))` gives it, the default summary otherwise.
fn api_args(cfg: &CrudDeclaration, op: CrudOp, summary: String) -> TokenStream2 {
    let api = cfg.api(op);
    let summary = match api.and_then(|api| api.summary.as_ref()) {
        Some(written) => quote!(#written),
        None => quote!(#summary),
    };
    let description = api
        .and_then(|api| api.description.as_ref())
        .map(|description| quote!(, description = #description));
    quote!(summary = #summary #description)
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
            service = ThingsService, ops = [list, get, delete]
        });
        assert!(out.contains("fn list"), "list expected: {out}");
        assert!(out.contains("fn get"), "get expected: {out}");
        assert!(out.contains("fn delete"), "delete expected: {out}");
        assert!(!out.contains("fn create"), "create must be absent: {out}");
        assert!(!out.contains("fn update"), "update must be absent: {out}");
    }

    #[test]
    fn the_create_answers_a_created_and_the_cursor_list_a_paginated_answer() {
        let out = generated_methods(quote! {
            service = ThingsService
        });
        assert!(out.contains(":: nest_rs_http :: Created <"), "{out}");
        assert!(out.contains(":: nest_rs_http :: Paginated <"), "{out}");
        assert!(
            !out.contains("http_code (201)"),
            "the answer says its status: {out}"
        );
        let whole = generated_methods(quote! {
            service = ThingsService, ops = [list], paginate = none
        });
        assert!(
            !whole.contains("Paginated"),
            "a full collection sends no cursor: {whole}",
        );
    }

    // An attribute, never a parameter: `#[routes]` runs the class gate with the
    // guards, outside the route's own layers.
    #[test]
    fn every_op_declares_its_posture_with_authorize() {
        let out = generated_methods(quote! {
            service = ThingsService
        });
        for action in ["Read", "Create", "Update", "Delete"] {
            assert!(
                out.contains(&format!(
                    "# [authorize (:: nest_rs_authz :: {action} , __NestrsCrudThingsEntity)]"
                )),
                "{action}: {out}",
            );
        }
        assert!(!out.contains("Authorize <"), "no posture parameter: {out}");
    }

    // RFC 5789 makes `PATCH` a partial update; `Update<E>` carries every
    // writable field, so the route replaces them (RFC 9110 §9.3.4).
    #[test]
    fn the_update_op_replaces_the_row_through_put() {
        let out = generated_methods(quote! {
            service = ThingsService, ops = [update]
        });
        assert!(out.contains("# [put (\"/{id}\")]"), "{out}");
        assert!(!out.contains("patch"), "{out}");
    }

    #[test]
    fn api_documents_the_operation_it_names_and_leaves_the_others_their_default() {
        let out = generated_methods(quote! {
            service = ThingsService, ops = [list, get],
            api(get(summary = "Get a thing in the caller's org", description = "Absent: 404.")),
        });
        assert!(
            out.contains(
                "# [api (summary = \"Get a thing in the caller's org\" , description = \"Absent: 404.\")]"
            ),
            "{out}",
        );
        assert!(out.contains("summary = \"List Thing\""), "{out}");
    }

    #[test]
    fn api_naming_an_op_ops_leaves_out_fails_to_expand() {
        let item: ItemImpl = parse_quote! { impl Things {} };
        let err = crud(
            quote! { service = ThingsService, ops = [list], api(get(summary = "x")) },
            item,
        )
        .expect_err("docs for an op that is not generated must fail");
        let err = err.to_string();
        assert!(
            err.contains("`get`") && err.contains("`ops` leaves out"),
            "{err}"
        );
    }

    #[test]
    fn a_key_configuring_an_op_the_block_writes_by_hand_fails_to_expand() {
        for (args, refusal) in [
            (
                quote! {
                    service = ThingsService, ops = [list, get],
                    api(get(summary = "Get a thing")),
                },
                "#[crud] `api` configures the `get` op, which this block writes by hand as \
                 `fn get` — drop `get(…)`: give that method its own `#[api]`",
            ),
            (
                quote! { service = ThingsService, paginate = none },
                "#[crud] `paginate` configures the `list` op, which this block writes by hand \
                 as `fn list` — drop `paginate = …`: the method bounds its own list",
            ),
        ] {
            let item: ItemImpl = parse_quote! {
                impl Things {
                    async fn list(&self) {}
                    async fn get(&self) {}
                }
            };
            let err = crud(args, item).expect_err("a key no generated op reads is refused");
            assert_eq!(err.to_string(), refusal);
        }
    }

    #[test]
    fn the_service_alone_generates_every_route_typed_off_the_service() {
        let out = generated_methods(quote! { service = ThingsService });
        for method in ["fn list", "fn get", "fn create", "fn update", "fn delete"] {
            assert!(out.contains(method), "{method}: {out}");
        }
        for alias in [
            "fn __nestrs_crud_things_service",
            "type __NestrsCrudThingsEntity",
            "type __NestrsCrudThingsOutput",
            "type __NestrsCrudThingsCreate",
            "type __NestrsCrudThingsUpdate",
        ] {
            assert!(out.contains(alias), "{alias}: {out}");
        }
        assert!(
            out.contains("summary = \"List Thing\""),
            "the noun is read off the host: {out}"
        );
    }
}
