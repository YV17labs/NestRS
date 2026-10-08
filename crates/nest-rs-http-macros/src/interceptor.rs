//! `#[interceptor]` — mark a struct as a **global** HTTP interceptor, mounted
//! as an `nest_rs_http::HttpEndpointWrap` rather than provided.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    from_container_method, injected_method, injected_optional_method, optional_dependencies_method,
};

const INTERCEPTOR: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("interceptor", &["priority"]);

fn parse_priority(args: TokenStream) -> syn::Result<TokenStream2> {
    if args.is_empty() {
        return Ok(quote! { ::nest_rs_http::endpoint_wrap_priority::INTERCEPTORS });
    }
    let mut priority: Option<syn::Expr> = None;
    INTERCEPTOR.parse2(TokenStream2::from(args), |arg| {
        priority = Some(arg.expr()?);
        Ok(())
    })?;
    let Some(written) = priority else {
        return Ok(quote! { ::nest_rs_http::endpoint_wrap_priority::INTERCEPTORS });
    };
    let priority = priority_value(&written)?;
    Ok(quote! { #priority })
}

/// A `priority = …` value: an integer literal in `i32`'s range, negative ones
/// included. syn hands a lone `-10` over as a negative literal and one followed
/// by another argument as a negation, so both shapes are read.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn priority_value(written: &syn::Expr) -> syn::Result<i32> {
    use syn::{Expr, ExprLit, ExprUnary, Lit, UnOp};

    let written = nest_rs_codegen::ungrouped_expr(written);
    let refused = || {
        syn::Error::new_spanned(
            written,
            nest_rs_codegen::takes_value(
                "interceptor",
                Some("priority"),
                "an integer literal in `i32`'s range, e.g. `priority = 10` — a lower priority \
                 wraps closer to the handler",
            ),
        )
    };
    let (negated, literal) = match written {
        Expr::Lit(ExprLit {
            lit: Lit::Int(literal),
            ..
        }) => (false, literal),
        Expr::Unary(ExprUnary {
            op: UnOp::Neg(_),
            expr,
            ..
        }) => match nest_rs_codegen::ungrouped_expr(expr) {
            Expr::Lit(ExprLit {
                lit: Lit::Int(literal),
                ..
            }) => (true, literal),
            _ => return Err(refused()),
        },
        _ => return Err(refused()),
    };
    let magnitude: i64 = literal.base10_parse().map_err(|_| refused())?;
    let value = if negated { -magnitude } else { magnitude };
    i32::try_from(value).map_err(|_| refused())
}

pub(crate) fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    let priority = match parse_priority(args) {
        Ok(p) => p,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
        ..
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let dependencies = dependencies_method(&dep_keys);
    let dependency_names = dependency_names_method(&dep_names);
    let optional_dependencies = optional_dependencies_method(&opt_keys);
    let injected = injected_method(&dep_keys);
    let injected_optional = injected_optional_method(&opt_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected
            #injected_optional

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                let __snapshot = builder.snapshot();
                let __value = Self::from_container(&__snapshot);
                let __arc: ::std::sync::Arc<dyn ::nest_rs_interceptors::Interceptor> =
                    ::std::sync::Arc::new(__value);
                builder.attach_meta::<Self, ::nest_rs_http::HttpEndpointWrap>(
                    ::nest_rs_http::HttpEndpointWrap::with_priority(
                        #priority,
                        move |_container, __endpoint| {
                        ::nest_rs_http::poem::EndpointExt::boxed(::nest_rs_http::poem::EndpointExt::map_to_response(
                            ::nest_rs_interceptors::InterceptorExt::interceptor(
                                __endpoint,
                                ::std::sync::Arc::clone(&__arc),
                            ),
                        ))
                    },
                    ),
                )
            }
        }
    }
    .into()
}
