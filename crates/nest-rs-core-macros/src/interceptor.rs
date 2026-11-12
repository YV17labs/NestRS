use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::Expr;

use nest_rs_codegen::__private::{key_as_written, parse_provider_host, unknown_value};

use crate::injectable::{InjectableScope, provider};

pub(crate) fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    let expanded = parse_stage(args.into()).and_then(|()| {
        let item = parse_provider_host(input.into())?;
        let registered = quote! {
            ::nest_rs_core::__private::register_interceptor::<Self>(builder)
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

/// The refusal of the stage the attribute does not declare, bare or spelled.
const HANDLER_STAGE: &str = "#[interceptor] declares what a module brings around the guards and \
     takes `stage = unit`: an app-wide interceptor at the handler stage is the root's — write \
     `#[injectable]` on the struct and list it in `GlobalInterceptors<(…)>`, in the providers of \
     the module the app boots";

/// `priority` is the old HTTP attribute's key: here an interceptor orders
/// itself through `#[derive(Layer)]`'s `#[layer(priority = …)]`.
fn known_elsewhere(key: &str) -> Option<String> {
    (key == "priority").then(|| {
        "#[interceptor] takes no `priority`: an interceptor orders itself within its stage \
         through `Layer::priority` — derive `Layer` with `#[layer(priority = …)]`"
            .to_owned()
    })
}

/// `#[interceptor(…)]`'s one sentence, `stage = unit`; anything else refused.
fn parse_stage(args: TokenStream2) -> syn::Result<()> {
    if args.is_empty() {
        return Err(syn::Error::new(Span::call_site(), HANDLER_STAGE));
    }
    INTERCEPTOR.parse2(args, |arg| {
        let value = arg.expr()?;
        let refused = |written: &str| {
            syn::Error::new_spanned(
                &value,
                unknown_value("interceptor", "stage", written, &["unit"]),
            )
        };
        let Expr::Path(path) = &value else {
            return Err(refused(&quote!(#value).to_string()));
        };
        match key_as_written(&path.path).as_str() {
            "unit" => Ok(()),
            "handler" => Err(syn::Error::new_spanned(&value, HANDLER_STAGE)),
            other => Err(refused(other)),
        }
    })
}
