use nest_rs_codegen::Grammar;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ItemStruct, LitStr, parse_macro_input};

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

    // Without `crate = ` the derive emits `::validator::`, which the call site
    // would have to depend on.
    let derive = (!manual_validate).then(|| {
        quote! {
            #[derive(::nest_rs_config::validator::Validate)]
            #[validate(crate = ::nest_rs_config::validator)]
        }
    });

    // The literal and the declaration, not the type: a generic config files one entry.
    let declaration = quote! {
        ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#name))
    };
    quote! {
        #derive
        #item

        impl #impl_generics ::nest_rs_config::Namespaced for #name #ty_generics #where_clause {
            const NAMESPACE: &'static str = #namespace_lit;
            const DECLARATION: &'static str = #declaration;
        }

        ::nest_rs_config::inventory::submit! {
            ::nest_rs_config::ConfigNamespace::new(#namespace_lit, #declaration)
        }
    }
    .into()
}

struct Args {
    namespace: LitStr,
    manual_validate: bool,
}

/// Every key `#[config]` takes, in the order its refusals list them.
const CONFIG: Grammar = Grammar::new("config", &["namespace", "validate"]);

fn parse_args(args: TokenStream2) -> syn::Result<Args> {
    let mut namespace: Option<LitStr> = None;
    let mut manual_validate = false;
    CONFIG.parse2(args, |arg| {
        match arg.key() {
            "namespace" => namespace = Some(arg.str_lit("seaorm")?),
            "validate" => {
                let lit = arg.str_lit("manual")?;
                if lit.value() != "manual" {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        "#[config] `validate` takes only `\"manual\"`, which suppresses the \
                         derive so the struct can write `impl Validate` itself",
                    ));
                }
                manual_validate = true;
            }
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;

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

/// Lowercase env-domain segment. It does not round-trip: `_` is admitted, so
/// `("social__google", "CLIENT_ID")` and `("social", "GOOGLE__CLIENT_ID")` name
/// one variable — the claim registry keys on the resolved name for that.
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
             e.g. \"seaorm\" or \"redis__queue\"",
        ))
    }
}
