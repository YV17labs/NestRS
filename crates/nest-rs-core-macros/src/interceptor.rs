use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Expr;

use nest_rs_codegen::parse_provider_host;

use crate::injectable::{InjectableScope, provider};

pub(crate) fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    let expanded = parse_stage(args.into()).and_then(|stage| {
        let item = parse_provider_host(input.into())?;
        let registered = quote! {
            ::nest_rs_core::__private::register_interceptor::<Self>(
                builder,
                ::nest_rs_core::__private::InterceptorStage::#stage,
            )
        };
        provider(item, InjectableScope::Singleton, registered)
    });
    match expanded {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

const INTERCEPTOR: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("interceptor", &["stage"]).elsewhere(known_elsewhere);

/// The stages as `stage = …` spells them, in the order a refusal lists them.
const STAGES: [&str; 2] = ["unit", "handler"];

/// `priority` is the old HTTP attribute's key: here an interceptor orders
/// itself through its `Layer` impl.
fn known_elsewhere(key: &str) -> Option<String> {
    (key == "priority").then(|| {
        "#[interceptor] takes no `priority`: an interceptor orders itself within its stage \
         through `Layer::priority` — override it on the struct's `Layer` impl"
            .to_owned()
    })
}

/// The stage `#[interceptor(…)]` declares, as the variant of
/// `InterceptorStage` it names; `handler` when unwritten.
fn parse_stage(args: TokenStream2) -> syn::Result<syn::Ident> {
    let mut stage = "Handler";
    INTERCEPTOR.parse2(args, |arg| {
        let value = arg.expr()?;
        let refused = |written: &str| {
            syn::Error::new_spanned(
                &value,
                nest_rs_codegen::unknown_value("interceptor", "stage", written, &STAGES),
            )
        };
        let Expr::Path(path) = &value else {
            return Err(refused(&quote!(#value).to_string()));
        };
        stage = match nest_rs_codegen::key_as_written(&path.path).as_str() {
            "unit" => "Unit",
            "handler" => "Handler",
            other => return Err(refused(other)),
        };
        Ok(())
    })?;
    Ok(syn::Ident::new(stage, proc_macro2::Span::call_site()))
}
