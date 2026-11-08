//! Emit the wire output object, its `From<&Model>` and the entity's `Exposed`
//! impl naming it — the one association from an entity to its wire type,
//! which `#[crud]` and the relation resolvers read. Only `#[expose]`d
//! columns appear — an unexposed field is absent; a `Uuid` or a timestamp
//! renders as a `String` on the wire, its schema keeping the `uuid` or
//! `date-time` format.
//!
//! **The row is judged where it is built**: `From<&Model>` asks the ability
//! installed there which columns the caller may read, so a column a grant
//! withholds is never in the wire row — an `Option` column, or one
//! `#[expose(nullable)]` declares, reads `None` — and the row carries who judged
//! it, which every serializer and GraphQL column checks, so it answers that caller
//! alone. A private proof field keeps a struct literal from building one, and the
//! type has no `Deserialize`: nothing builds a wire row but the judgement.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::Ident;
use syn::ext::IdentExt as _;

use crate::attr::{
    ResourceField, ResourceModel, complexity_attr, graphql_crate_attr, graphql_object_derive,
    is_datetime_tz, is_uuid,
};

/// The format a `String` wire field keeps from its column's type.
fn wire_format(field: &ResourceField) -> Option<&'static str> {
    if is_uuid(&field.ty) {
        Some("uuid")
    } else if is_datetime_tz(&field.ty) {
        Some("date-time")
    } else {
        None
    }
}

/// The wire value of `field`, read off `model`.
fn wire_value(field: &ResourceField) -> TokenStream2 {
    let name = &field.ident;
    if is_uuid(&field.ty) {
        quote! { ::std::string::ToString::to_string(&model.#name) }
    } else if is_datetime_tz(&field.ty) {
        // chrono's own `Serialize` spelling (`Z` for UTC): one instant, one string.
        quote! {
            ::nest_rs_seaorm::__private::chrono::DateTime::<::nest_rs_seaorm::__private::chrono::FixedOffset>::to_rfc3339_opts(
                &model.#name,
                ::nest_rs_seaorm::__private::chrono::SecondsFormat::AutoSi,
                true,
            )
        }
    } else {
        quote! { ::core::clone::Clone::clone(&model.#name) }
    }
}

/// The schema function a nullable field's `#[schemars(schema_with)]` names: its
/// value's schema or `null`, in `format`, the key required, since a reply always
/// writes it.
fn nullable_schema_fn(
    field: &ResourceField,
    base: &TokenStream2,
    format: Option<&str>,
) -> (Ident, TokenStream2) {
    let ident = format_ident!("__nullable_schema_{}", field.ident.unraw());
    let format = match format {
        Some(format) => quote! { ::core::option::Option::Some(#format) },
        None => quote! { ::core::option::Option::None },
    };
    let body = quote! {
        #[doc(hidden)]
        fn #ident(
            __generator: &mut ::nest_rs_seaorm::__private::schemars::SchemaGenerator,
        ) -> ::nest_rs_seaorm::__private::schemars::Schema {
            ::nest_rs_seaorm::__private::nullable_schema::<#base>(__generator, #format)
        }
    };
    (ident, body)
}

pub(crate) fn emit(model: &ResourceModel) -> TokenStream2 {
    let output = &model.output_ident;
    let source = &model.source_ident;
    let output_name = output.to_string();
    let mut decls = Vec::new();
    let mut inits = Vec::new();
    let mut keys = Vec::new();
    let mut serialized = Vec::new();
    let mut schema_fns = Vec::new();
    let mut answered = Vec::new();

    for field in model.fields.iter().filter(|f| f.in_output_struct()) {
        let name = &field.ident;
        let key = field.key();
        let complexity = if model.graphql {
            complexity_attr(&field.complexity, None)
        } else {
            TokenStream2::new()
        };
        let guard = if model.graphql {
            let key = field.is_pk;
            quote! {
                #[graphql(guard = ::nest_rs_authz::__private::judged_field::<Entity>(self.__judged, #key))]
            }
        } else {
            TokenStream2::new()
        };
        let format = wire_format(field);
        let base = if format.is_some() {
            quote! { ::std::string::String }
        } else {
            let ty = &field.ty;
            quote! { #ty }
        };
        let value = wire_value(field);
        let withheld = field.column.as_ref().filter(|_| field.withholdable());
        match (field.nullable, withheld) {
            (true, Some(column)) => {
                let (schema_fn, body) = nullable_schema_fn(field, &base, format);
                let schema_with = format!("{output}::{schema_fn}");
                schema_fns.push(body);
                decls.push(quote! {
                    #complexity
                    #guard
                    #[schemars(schema_with = #schema_with)]
                    pub #name: ::core::option::Option<#base>
                });
                inits.push(quote! {
                    #name: __judgement.permits(Column::#column).then(|| #value)
                });
            }
            (false, Some(column)) => {
                decls.push(quote! { #complexity #guard pub #name: #base });
                inits.push(quote! {
                    #name: if __judgement.permits(Column::#column) {
                        #value
                    } else {
                        ::core::option::Option::None
                    }
                });
            }
            (_, None) => {
                let format = format.map(|format| {
                    quote! { #[schemars(extend("format" = #format))] }
                });
                decls.push(quote! { #complexity #guard #format pub #name: #base });
                inits.push(quote! { #name: #value });
                if let Some(column) = &field.column {
                    answered.push(quote! {
                        (
                            <Column as ::nest_rs_seaorm::sea_orm::IdenStatic>::as_str(&Column::#column),
                            #key,
                        )
                    });
                }
            }
        }
        serialized.push(quote! {
            ::nest_rs_seaorm::__private::serde::ser::SerializeStruct::serialize_field(
                &mut __row, #key, &self.#name,
            )?;
        });
        keys.push(key);
    }
    let width = keys.len();

    let complex = if model.graphql && model.complex {
        quote! { #[graphql(complex)] }
    } else {
        quote! {}
    };

    let graphql_derives = graphql_object_derive(model, "SimpleObject");
    let graphql_crate = graphql_crate_attr(model);
    let graphql_skip = model.graphql.then(|| quote! { #[graphql(skip)] });

    quote! {
        // A derive expands against the call site's prelude: without `crate = `
        // the entity crate would have to declare `schemars`.
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            #graphql_derives
            ::nest_rs_seaorm::__private::schemars::JsonSchema,
        )]
        #[schemars(crate = "::nest_rs_seaorm::__private::schemars")]
        #graphql_crate
        #complex
        pub struct #output {
            #(#decls,)*
            #[schemars(skip)]
            #graphql_skip
            __judged: ::nest_rs_authz::__private::Judged,
        }

        impl #output {
            #(#schema_fns)*
        }

        impl ::core::convert::From<&#source> for #output {
            fn from(model: &#source) -> Self {
                let __judgement = ::nest_rs_authz::__private::judge::<Entity>(model);
                Self {
                    #(#inits,)*
                    __judged: __judgement.proof(),
                }
            }
        }

        // Every key is written, a withheld one as `null`, so the schema's `required`
        // holds; the row first proves it answers the caller being written for.
        impl ::nest_rs_seaorm::__private::serde::Serialize for #output {
            fn serialize<__S: ::nest_rs_seaorm::__private::serde::Serializer>(
                &self,
                __serializer: __S,
            ) -> ::core::result::Result<__S::Ok, __S::Error> {
                if let ::core::result::Result::Err(__refused) =
                    self.__judged.serialize_check::<Entity>()
                {
                    return ::core::result::Result::Err(
                        <__S::Error as ::nest_rs_seaorm::__private::serde::ser::Error>::custom(
                            __refused,
                        ),
                    );
                }
                let mut __row = ::nest_rs_seaorm::__private::serde::Serializer::serialize_struct(
                    __serializer, #output_name, #width,
                )?;
                #(#serialized)*
                ::nest_rs_seaorm::__private::serde::ser::SerializeStruct::end(__row)
            }
        }

        impl ::nest_rs_authz::__private::JudgedRow for #output {}

        impl ::nest_rs_authz::__private::WireRow for #output {
            fn refusal(&self) -> ::core::option::Option<::nest_rs_authz::__private::RefusedAnswer> {
                self.__judged.refusal::<Entity>()
            }
        }

        ::nest_rs_authz::__private::inventory::submit! {
            ::nest_rs_authz::__private::WireColumns {
                entity: ::core::any::TypeId::of::<Entity>,
                answered: || ::std::vec![#(#answered),*],
            }
        }

        impl ::nest_rs_seaorm::__private::ExposeDeclared for Entity {}

        impl ::nest_rs_seaorm::Exposed for Entity {
            type Wire = #output;
        }
    }
}
