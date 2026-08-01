use nest_rs_codegen::require_str_lit;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{ItemStruct, LitStr, MetaNameValue, Token, parse_macro_input};

pub(crate) fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    let Args {
        namespace,
        manual_validate,
    } = match parse_args(args.into()) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    let item = parse_macro_input!(input as ItemStruct);
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let namespace_lit = namespace.value();

    // The decorator carries `Validate` and points it back at the framework's
    // own copy. Without the `crate = ` override the derive would emit
    // `::validator::` against the *call site's* prelude, which is what used to
    // force `validator = "0.20"` — and an exact version to align — into the
    // manifest of every crate holding a `#[config]` struct.
    //
    // `validate = "manual"` opts out, for the config that validates across
    // fields and writes the impl by hand: deriving on top of that is a
    // conflicting impl, and cross-field rules are a real need, not a mistake.
    let derive = (!manual_validate).then(|| {
        quote! {
            #[derive(::nest_rs_config::validator::Validate)]
            #[validate(crate = ::nest_rs_config::validator)]
        }
    });

    quote! {
        #derive
        #item

        impl #impl_generics ::nest_rs_config::Namespaced for #name #ty_generics #where_clause {
            const NAMESPACE: &'static str = #namespace_lit;
        }
    }
    .into()
}

// Deliberately bespoke rather than `nest_rs_codegen::parse_named_str_arg`: that
// shared helper parses a single `key = "..."` and cannot name an *unexpected*
// argument, whereas `#[config]` rejects unknown keys by name (see the `other`
// arm below). The friendlier diagnostic is worth the local parser.
struct Args {
    namespace: LitStr,
    manual_validate: bool,
}

fn parse_args(args: TokenStream2) -> syn::Result<Args> {
    let metas = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(args)?;

    let mut namespace: Option<LitStr> = None;
    let mut manual_validate = false;
    for meta in metas {
        let key = meta
            .path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_default();
        match key.as_str() {
            "namespace" => {
                namespace = Some(require_str_lit(
                    &meta.value,
                    "config",
                    "namespace",
                    "database",
                )?)
            }
            "validate" => {
                let lit = require_str_lit(&meta.value, "config", "validate", "manual")?;
                if lit.value() != "manual" {
                    return Err(syn::Error::new_spanned(
                        &meta.value,
                        "#[config] `validate` takes only `\"manual\"`, which suppresses the \
                         derive so the struct can write `impl Validate` itself",
                    ));
                }
                manual_validate = true;
            }
            other => {
                return Err(syn::Error::new_spanned(
                    &meta.path,
                    format!(
                        "unknown #[config] argument `{other}`; expected `namespace` or `validate`"
                    ),
                ));
            }
        }
    }

    let lit = namespace.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[config] needs a namespace: `#[config(namespace = \"database\")]`",
        )
    })?;
    validate_namespace(&lit)?;
    Ok(Args {
        namespace: lit,
        manual_validate,
    })
}

/// Lowercase env-domain segment so it round-trips into `NESTRS_<DOMAIN>__`.
fn validate_namespace(lit: &LitStr) -> syn::Result<()> {
    let value = lit.value();
    let valid = !value.is_empty()
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            lit.span(),
            "#[config] `namespace` must be a lowercase env-domain segment \
             (start with a letter, then lowercase letters, digits, or underscores), \
             e.g. \"database\" or \"object_store\"",
        ))
    }
}
