//! `#[meta(EXPR)]` — the values a host or a dispatched method declares — and
//! the one `nest_rs_core::Handler` each dispatch site's declaration builds.
//!
//! Every impl half reads `#[meta]` the same way, at two levels: the host's (on
//! the struct a pair's host half decorates, or on the impl block a provider
//! pair decorates) and the method's. A site's `Handler` holds both, built once
//! on its first read; a read prefers the method's value of a type.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Attribute, Expr, Ident, PathSegment, Type};

use crate::args::takes_value;
use crate::pair::DecoratorPair;

/// The attribute this module reads.
const META: &str = "meta";

/// Every `#[meta(EXPR)]` on an item, taken off it in the order written.
///
/// Two values whose expression names one type — `Throttle(10)` and
/// `Throttle { limit: 20 }`, the same unit struct twice, or two variants of one
/// enum — are refused at the second: a read takes the first of a type, so the
/// second would be read by nothing. An expression that names no type (a call
/// to a function, a method call) cannot be told apart here.
pub fn take_meta(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Expr>> {
    let mut values: Vec<Expr> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident(META)) {
        let value = meta_value(attr)?;
        if let Some(ty) = named_type(&value) {
            if named.contains(&ty) {
                return Err(syn::Error::new_spanned(attr, meta_written_twice(&ty)));
            }
            named.push(ty);
        }
        values.push(value);
    }
    attrs.retain(|attr| !attr.path().is_ident(META));
    Ok(values)
}

/// `#[meta]`'s one argument, the value, read as an expression.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn meta_value(attr: &Attribute) -> syn::Result<Expr> {
    let refused = || {
        syn::Error::new_spanned(
            attr,
            takes_value(
                META,
                None,
                "one value as an expression, e.g. `#[meta(Throttle::per_minute(10))]`",
            ),
        )
    };
    let list = attr.meta.require_list().map_err(|_| refused())?;
    syn::parse2::<Expr>(list.tokens.clone()).map_err(|_| refused())
}

/// The type a value's expression names by its own path, as written: a tuple
/// struct called (`Throttle(10)`), a struct literal (`Throttle { .. }`), a unit
/// struct (`SkipThrottle`), or an enum through one of its variants (`Role::Admin`
/// and `Role::Grant(..)` both name `Role`). `None` where the path names a
/// function, a constant or a variant of `Option` or `Result`, whose type the
/// expression does not show.
fn named_type(value: &Expr) -> Option<String> {
    let path = match crate::ungrouped_expr(value) {
        Expr::Call(call) => match crate::ungrouped_expr(&call.func) {
            Expr::Path(path) if path.qself.is_none() => &path.path,
            _ => return None,
        },
        Expr::Struct(literal) if literal.qself.is_none() => &literal.path,
        Expr::Path(path) if path.qself.is_none() => &path.path,
        _ => return None,
    };
    let mut segments: Vec<&PathSegment> = path.segments.iter().collect();
    let last = segments.pop()?;
    if !names_a_type(last) || GENERIC_PRELUDE.contains(&&*last.ident.unraw().to_string()) {
        return None;
    }
    // A variant: the segment before it names its enum.
    let named = match segments.last() {
        Some(parent) if names_a_type(parent) => segments,
        _ => path.segments.iter().collect(),
    };
    if named
        .last()
        .is_some_and(|ty| GENERIC_PRELUDE.contains(&&*ty.ident.unraw().to_string()))
    {
        return None;
    }
    Some(
        named
            .iter()
            .map(|segment| segment.ident.unraw().to_string())
            .collect::<Vec<_>>()
            .join("::"),
    )
}

/// The prelude's generic enums and their variants: two values written through
/// them may hold two distinct types (`Some(1_u32)`, `Some("a")`).
const GENERIC_PRELUDE: [&str; 6] = ["Option", "Some", "None", "Result", "Ok", "Err"];

/// Whether a segment is spelled as a type or a variant is (`UpperCamelCase`),
/// not as a function, a module or a constant.
fn names_a_type(segment: &PathSegment) -> bool {
    let name = segment.ident.unraw().to_string();
    name.chars().next().is_some_and(char::is_uppercase) && name.chars().any(char::is_lowercase)
}

/// The sentence a second `#[meta]` of one type is refused with.
fn meta_written_twice(ty: &str) -> String {
    format!(
        "#[meta] declares one `{ty}` here, and this is a second — a read takes the first value \
         of a type, so this one would be read by nothing. Keep one; a method's own value \
         already overrides its host's"
    )
}

/// Refuse a `#[meta]` written on an edge pair's operations block, naming the
/// struct half where a value for the whole host is declared.
pub fn reject_meta_on_operations(pair: &DecoratorPair, attrs: &[Attribute]) -> syn::Result<()> {
    match attrs.iter().find(|attr| attr.path().is_ident(META)) {
        Some(attr) => Err(syn::Error::new_spanned(
            attr,
            format!(
                "`#[meta(...)]` for the whole host belongs on the struct beside `{host}`, not \
                 on this `{operations}` block; a value for one operation goes on its method",
                host = pair.host(),
                operations = pair.operations(),
            ),
        )),
        None => Ok(()),
    }
}

/// The hidden function a pair's host half emits on the host, returning the
/// host-level `#[meta]` values each of its sites reads.
pub fn host_meta_fn(values: &[Expr]) -> TokenStream {
    let values = boxed(values);
    quote! {
        #[doc(hidden)]
        pub fn __nestrs_host_meta() -> ::std::vec::Vec<
            ::std::boxed::Box<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
        > {
            #values
        }
    }
}

/// Where a site's host-level values come from.
pub enum HostMeta<'a> {
    /// The host half's [`host_meta_fn`], on the struct.
    Struct,
    /// The impl block a provider pair decorates.
    Values(&'a [Expr]),
}

/// Who a site's declaration says may call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SitePosture {
    /// `#[public]`.
    Public,
    /// `#[authorize]`, or a site gated by its guards alone.
    Gated,
    /// No caller: a queue attempt, a scheduled tick, an event listener.
    System,
}

/// One dispatch site, as its impl half declares it.
pub struct HandlerSite<'a> {
    /// The path of the unit the site runs as, e.g. `::nest_rs_http::unit::REQUEST`.
    pub unit: TokenStream,
    /// The host's type as the impl block names it.
    pub self_ty: &'a Type,
    /// The host's name, as written.
    pub host_name: &'a Ident,
    /// The dispatched method.
    pub method: &'a Ident,
    /// What a client addresses, a `&'static str` expression.
    pub address: TokenStream,
    /// What the site's posture declared.
    pub posture: SitePosture,
    /// The host-level values.
    pub host_meta: HostMeta<'a>,
    /// The method's values.
    pub method_meta: &'a [Expr],
    /// The method's `#[cfg]` conditions.
    pub cfgs: &'a [TokenStream],
}

/// The hidden associated items declaring a site, placed in an inherent `impl`
/// of the host: the const holding its `&'static Handler`, which
/// [`handler_ref`] reads, and the function building its `#[meta]` values, an
/// associated one so that `Self` in a value names the host.
pub fn handler_const(site: &HandlerSite<'_>) -> TokenStream {
    let HandlerSite {
        unit,
        self_ty,
        host_name,
        method,
        address,
        posture,
        host_meta,
        method_meta,
        cfgs,
    } = site;
    let name = handler_ident(method);
    // Cased as a function's is: a method may be named in any case.
    let meta_fn = format_ident!(
        "__nestrs_meta_{}",
        method.unraw().to_string().to_lowercase()
    );
    let host_name = host_name.unraw().to_string();
    let method_name = method.unraw().to_string();
    let posture = match posture {
        SitePosture::Public => quote!(Public),
        SitePosture::Gated => quote!(Gated),
        SitePosture::System => quote!(System),
    };
    let host_values = match host_meta {
        HostMeta::Struct => quote! { <#self_ty>::__nestrs_host_meta() },
        HostMeta::Values(values) => boxed(values),
    };
    let method_values = boxed(method_meta);
    quote! {
        #(#cfgs)*
        #[doc(hidden)]
        fn #meta_fn() -> ::nest_rs_core::__private::MetaLevels {
            ::nest_rs_core::__private::MetaLevels {
                host: #host_values,
                method: #method_values,
            }
        }

        #(#cfgs)*
        #[doc(hidden)]
        #[allow(
            dead_code,
            reason = "a site is declared whether or not its edge reads it, and its values stay built"
        )]
        pub const #name: &'static ::nest_rs_core::Handler = {
            static __NESTRS_HANDLER: ::nest_rs_core::Handler =
                ::nest_rs_core::__private::declare_handler(
                    ::nest_rs_core::__private::HandlerDeclaration {
                        unit: #unit,
                        host: ::core::any::TypeId::of::<#self_ty>(),
                        host_name: #host_name,
                        method: #method_name,
                        address: #address,
                        posture: ::nest_rs_core::Posture::#posture,
                        meta: <#self_ty>::#meta_fn,
                    },
                );
            &__NESTRS_HANDLER
        };
    }
}

/// The `&'static Handler` of `method`'s site on `self_ty`.
pub fn handler_ref(self_ty: &Type, method: &Ident) -> TokenStream {
    let name = handler_ident(method);
    quote! { <#self_ty>::#name }
}

/// The name of a site's handler const: the method's, cased as a const's is.
fn handler_ident(method: &Ident) -> Ident {
    format_ident!(
        "__NESTRS_HANDLER_{}",
        method.unraw().to_string().to_uppercase()
    )
}

/// `values` as the boxed vector a level holds; each is required `Send + Sync +
/// 'static` where it is written.
fn boxed(values: &[Expr]) -> TokenStream {
    quote! {
        ::std::vec![#(::nest_rs_core::__private::meta_value(#values)),*]
    }
}

#[cfg(test)]
mod tests {
    use quote::ToTokens;
    use syn::parse_quote;

    use super::*;

    fn taken(attrs: &mut Vec<Attribute>) -> Result<Vec<String>, String> {
        take_meta(attrs)
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.to_token_stream().to_string())
                    .collect()
            })
            .map_err(|err| err.to_string())
    }

    #[test]
    fn every_value_is_taken_in_order_and_other_attributes_stay() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[meta(Throttle(10))]),
            parse_quote!(#[doc = "kept"]),
            parse_quote!(#[meta(Audit { trail: "posts" })]),
            parse_quote!(#[meta(SkipThrottle)]),
            parse_quote!(#[meta(limit_for("posts"))]),
        ];
        assert_eq!(
            taken(&mut attrs).expect("four values"),
            [
                "Throttle (10)",
                "Audit { trail : \"posts\" }",
                "SkipThrottle",
                "limit_for (\"posts\")"
            ],
        );
        assert_eq!(attrs.len(), 1, "the doc comment stays");
    }

    #[test]
    fn a_second_value_of_one_named_type_is_refused() {
        for second in [
            parse_quote!(#[meta(Throttle(20))]),
            parse_quote!(#[meta(Throttle { limit: 20 })]),
        ] {
            let mut attrs: Vec<Attribute> = vec![parse_quote!(#[meta(Throttle(10))]), second];
            let refused = taken(&mut attrs).expect_err("one type, twice");
            assert!(refused.contains("one `Throttle`"), "{refused}");
        }
    }

    // A refusal is emitted beside the item as written, so it keeps its attributes.
    #[test]
    fn a_refusal_leaves_the_items_attributes_as_written() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[derive(Clone)]),
            parse_quote!(#[meta(Throttle(10))]),
            parse_quote!(#[meta(Throttle(20))]),
        ];
        assert!(take_meta(&mut attrs).is_err());
        assert_eq!(attrs.len(), 3);
    }

    #[test]
    fn two_variants_of_one_enum_are_two_values_of_one_type() {
        for (first, second) in [
            (
                parse_quote!(#[meta(Role::Admin)]),
                parse_quote!(#[meta(Role::Auditor)]),
            ),
            (
                parse_quote!(#[meta(Role::Admin)]),
                parse_quote!(#[meta(Role::Grant("posts"))]),
            ),
            (
                parse_quote!(#[meta(auth::Role::Admin)]),
                parse_quote!(#[meta(auth::Role::Grant { scope: "posts" })]),
            ),
        ] {
            let mut attrs: Vec<Attribute> = vec![first, second];
            let refused = taken(&mut attrs).expect_err("one enum, twice");
            assert!(refused.contains("one `"), "{refused}");
            assert!(refused.contains("Role`"), "{refused}");
        }
    }

    #[test]
    fn a_variant_of_a_generic_prelude_enum_names_no_type() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[meta(Some(1_u32))]),
            parse_quote!(#[meta(Some("a"))]),
            parse_quote!(#[meta(None::<u8>)]),
            parse_quote!(#[meta(Ok::<u8, ()>(1))]),
            parse_quote!(#[meta(Err::<(), u8>(1))]),
            parse_quote!(#[meta(Option::Some(1_u64))]),
        ];
        assert_eq!(taken(&mut attrs).map(|values| values.len()), Ok(6));
    }

    #[test]
    fn values_of_distinct_types_or_of_unnamed_types_pass() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[meta(Throttle(10))]),
            parse_quote!(#[meta(Audit("posts"))]),
            parse_quote!(#[meta(limit(10))]),
            parse_quote!(#[meta(limit(20))]),
            parse_quote!(#[meta(LIMIT)]),
            parse_quote!(#[meta(Role::Admin)]),
            parse_quote!(#[meta(Scope::Read)]),
            parse_quote!(#[meta(Throttle::DEFAULT)]),
            parse_quote!(#[meta(Throttle::per_minute(10))]),
        ];
        assert_eq!(taken(&mut attrs).map(|values| values.len()), Ok(9));
    }

    #[test]
    fn a_meta_without_one_expression_is_refused_naming_the_attribute() {
        for attr in [parse_quote!(#[meta]), parse_quote!(#[meta(a, b)])] {
            let mut attrs: Vec<Attribute> = vec![attr];
            let refused = taken(&mut attrs).expect_err("no single value");
            assert!(refused.starts_with("#[meta] takes one value"), "{refused}");
        }
    }

    #[test]
    fn a_handler_const_is_named_for_its_method() {
        let ty: Type = parse_quote!(PostsController);
        let method: Ident = parse_quote!(r#type);
        assert_eq!(
            handler_ref(&ty, &method).to_string(),
            "< PostsController > :: __NESTRS_HANDLER_TYPE",
        );
    }
}
