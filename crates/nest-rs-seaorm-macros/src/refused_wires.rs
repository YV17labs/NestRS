//! Keep a row off every wire but its exposed type: a wire derive written on it is
//! refused where it is read, and the bounded impls emitted on it answer one that
//! cannot be read.

use nest_rs_codegen::__private::{last_segment_ident, site};
use nest_rs_codegen::Refusals;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{Attribute, Ident, Meta, Path, Token};

use crate::attr::{ResourceModel, graphql_root};

/// Every wire derive written on the row, refused at its path: `Serialize` and
/// `SimpleObject` write each column, a hidden one too, onto a wire no mask shapes,
/// `Deserialize` and `InputObject` read each one off a body no input validates,
/// and `JsonSchema` publishes each one's name and type. Read in `#[derive(…)]` and
/// in sea-orm's `model_attrs(…)` and `model_ex_attrs(…)`, which put a derive on the
/// `Model` and on the `ModelEx` beside it.
pub(crate) fn derive_refusals(attrs: &[Attribute], model: &ResourceModel) -> Option<TokenStream> {
    let row = model.source_ident.to_string();
    let mut refusals = Refusals::default();
    for attr in attrs {
        if attr.path().is_ident("derive") {
            if let Ok(paths) = attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
            {
                refuse_wire_derives(&mut refusals, &paths, &row, model);
            }
        } else if attr.path().is_ident("sea_orm") {
            // sea-orm refuses its own attribute's syntax; only a derive it
            // forwards is read here.
            let Ok(keys) = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            else {
                continue;
            };
            for key in keys {
                let Meta::List(list) = key else { continue };
                let target = if list.path.is_ident("model_attrs") {
                    row.clone()
                } else if list.path.is_ident("model_ex_attrs") {
                    format!("{row}Ex")
                } else {
                    continue;
                };
                let Ok(forwarded) =
                    list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                else {
                    continue;
                };
                for derive in forwarded {
                    if let Meta::List(derive) = derive
                        && derive.path.is_ident("derive")
                        && let Ok(paths) =
                            derive.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
                    {
                        refuse_wire_derives(&mut refusals, &paths, &target, model);
                    }
                }
            }
        }
    }
    refusals
        .finish()
        .err()
        .map(|refused| refused.to_compile_error())
}

/// Refuse each wire derive among `paths`, derived on `row`.
fn refuse_wire_derives(
    refusals: &mut Refusals,
    paths: &Punctuated<Path, Token![,]>,
    row: &str,
    model: &ResourceModel,
) {
    let exposed = &model.output_ident;
    let writes = "writes every column, hidden ones included, onto a wire";
    for path in paths {
        let derive = last_segment_ident(path);
        let what = match derive.to_string().as_str() {
            "Serialize" => {
                format!("{writes} — a reply answers `{exposed}` (`Json({exposed}::from(&model))`)")
            }
            "SimpleObject" => {
                format!("{writes} — a reply answers `{exposed}` (`{exposed}::from(&model)`)")
            }
            "Deserialize" | "InputObject" => format!(
                "reads every column, hidden ones included, off a body no input validates — a \
                 write takes `{create}` or `{update}`, the inputs \
                 `#[expose(input(create, update))]` declares",
                create = model.create_ident,
                update = model.update_ident,
            ),
            "JsonSchema" => format!(
                "publishes every column, hidden ones included, in the document — a reply's \
                 schema is `{exposed}`'s"
            ),
            _ => continue,
        };
        refusals.push(syn::Error::new_spanned(
            path,
            format!(
                "{}: `{row}` derives `{derive}`, which {what}: drop the derive",
                site("expose", None),
            ),
        ));
    }
}

/// The impl of every trait that carries a row across a wire, on the `Model` and on
/// the `ModelEx` `#[sea_orm::model]` writes beside it, bounded on what nothing
/// answers: using one reports the bound's sentence, and writing another, which a
/// derive above `#[expose]` or an `#[Object]` does, overlaps it.
pub(crate) fn emit(attrs: &[Attribute], model: &ResourceModel) -> TokenStream {
    let row = &model.source_ident;
    let on_model = emit_on(row, model);
    let on_model_ex = attrs
        .iter()
        .any(writes_a_model_ex)
        .then(|| emit_on(&format_ident!("{row}Ex"), model));
    quote!(#on_model #on_model_ex)
}

/// Whether `attr` is `#[sea_orm::model]` or `#[sea_orm::compact_model]`, read by its
/// last segment, since no value tells a macro that a `ModelEx` will exist.
fn writes_a_model_ex(attr: &Attribute) -> bool {
    let last = last_segment_ident(attr.path());
    last == "model" || last == "compact_model"
}

/// [`emit`]'s impls on `target`, under the bound `nest-rs-seaorm`'s `hidden_columns`
/// declares, and hidden: the row's rustdoc page would read a refusal as a capability
/// and print the bound's names.
fn emit_on(target: &Ident, model: &ResourceModel) -> TokenStream {
    let private = quote!(::nest_rs_seaorm::__private);
    let (source, output) = (&model.source_ident, &model.output_ident);
    let answer = quote!(#private::AModelCrossesNoWire<fn(#source) -> #output>);
    let bound = quote!(for<'__row> #private::HiddenColumns<'__row>: #answer);
    let refused = quote!(<#private::HiddenColumns<'static> as #answer>::refused());
    let graphql = cfg!(feature = "graphql").then(|| emit_graphql_on(target, &bound, &refused));
    quote! {
        #[doc(hidden)]
        impl #private::serde::Serialize for #target where #bound {
            fn serialize<__S: #private::serde::Serializer>(
                &self,
                _: __S,
            ) -> ::core::result::Result<__S::Ok, __S::Error> {
                #refused
            }
        }

        #[doc(hidden)]
        impl<'__de> #private::serde::Deserialize<'__de> for #target where #bound {
            fn deserialize<__D: #private::serde::Deserializer<'__de>>(
                _: __D,
            ) -> ::core::result::Result<Self, __D::Error> {
                #refused
            }
        }

        #[doc(hidden)]
        impl #private::schemars::JsonSchema for #target where #bound {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                #refused
            }

            fn json_schema(
                _: &mut #private::schemars::SchemaGenerator,
            ) -> #private::schemars::Schema {
                #refused
            }
        }

        #graphql
    }
}

/// async-graphql's two wire traits, under this crate's `graphql` feature and not the
/// key of `#[expose]`'s, which a row needs none of to take `#[derive(SimpleObject)]`
/// above it. With the feature off, an async-graphql the app names itself stays open
/// (`.claude/decisions/mask-provenance.md`, 2026-11-05).
fn emit_graphql_on(target: &Ident, bound: &TokenStream, refused: &TokenStream) -> TokenStream {
    let root = graphql_root();
    quote! {
        #[doc(hidden)]
        impl #root::OutputType for #target where #bound {
            fn type_name() -> ::std::borrow::Cow<'static, str> {
                #refused
            }

            fn create_type_info(_: &mut #root::registry::Registry) -> ::std::string::String {
                #refused
            }

            async fn resolve(
                &self,
                _: &#root::ContextSelectionSet<'_>,
                _: &#root::Positioned<#root::parser::types::Field>,
            ) -> #root::ServerResult<#root::Value> {
                #refused
            }
        }

        #[doc(hidden)]
        impl #root::InputType for #target where #bound {
            type RawValueType = Self;

            fn type_name() -> ::std::borrow::Cow<'static, str> {
                #refused
            }

            fn create_type_info(_: &mut #root::registry::Registry) -> ::std::string::String {
                #refused
            }

            fn parse(
                _: ::core::option::Option<#root::Value>,
            ) -> #root::InputValueResult<Self> {
                #refused
            }

            fn to_value(&self) -> #root::Value {
                #refused
            }

            fn as_raw_value(&self) -> ::core::option::Option<&Self::RawValueType> {
                #refused
            }
        }
    }
}
