use nest_rs_codegen::{key_as_written, once, require_str_lit, unknown_argument, unmatched_meta};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{ItemStruct, LitStr, Meta, Token, parse_macro_input};

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

// Deliberately bespoke. `nest-rs-codegen` carried a shared "parse the sole
// `key = \"...\"`" helper for a while and this was the one crate that evaluated
// it: such a parser reads the value and cannot name an *unexpected* argument,
// whereas `#[config]` rejects unknown keys by name (see the `other` arm below).
// Nothing else ever called it, so it went; the friendlier diagnostic is worth
// the local parser, and that is the reason rather than an oversight.
struct Args {
    namespace: LitStr,
    manual_validate: bool,
}

fn parse_args(args: TokenStream2) -> syn::Result<Args> {
    // `Meta`, not `MetaNameValue`: a bare `#[config(namespace)]` is a
    // `Meta::Path`, so parsing the narrower shape died on syn's `` expected `=` ``
    // — the sentence `needs_a_value` exists to replace, at the decorator every
    // configurable module in both workspaces writes.
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;

    let mut namespace: Option<LitStr> = None;
    let mut manual_validate = false;
    for meta in metas {
        let Meta::NameValue(meta) = &meta else {
            return Err(unmatched_meta("config", &meta, &["namespace", "validate"]));
        };
        let key = key_as_written(&meta.path);
        match key.as_str() {
            "namespace" => {
                once(namespace.is_some(), &meta.path, "config", "namespace")?;
                namespace = Some(require_str_lit(
                    &meta.value,
                    "config",
                    "namespace",
                    "seaorm",
                )?)
            }
            "validate" => {
                once(manual_validate, &meta.path, "config", "validate")?;
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
                    unknown_argument("config", other, &["namespace", "validate"]),
                ));
            }
        }
    }

    let lit = namespace.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            nest_rs_codegen::missing_argument("config", "namespace", "\"seaorm\""),
        )
    })?;
    validate_namespace(&lit)?;
    Ok(Args {
        namespace: lit,
        manual_validate,
    })
}

/// Lowercase env-domain segment, so it reaches `<PREFIX>_<DOMAIN>__` uppercased
/// and unambiguous.
///
/// **It does not round-trip, and the earlier wording said it did.** `_` is
/// admitted, so `#[config(namespace = "social__google")]` is legal and names the
/// same variable as `("social", "GOOGLE__CLIENT_ID")` — two key pairs, one
/// variable, and the tree ships both spellings because `nest-rs-social` uses the
/// separator as a nesting device. The consequence is recorded where it bites, on
/// `nest_rs_config`'s claim registry, which keys on the resolved *name* for
/// exactly this reason; it is restated here because this is the earliest site
/// that can see the fact, and it said the opposite of it.
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
             e.g. \"seaorm\" or \"redis__worker\"",
        ))
    }
}
