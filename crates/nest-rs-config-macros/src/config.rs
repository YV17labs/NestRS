use nest_rs_codegen::Grammar;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ItemStruct, LitStr, parse_macro_input};

use crate::overlay::{Fields, overlay};

pub(crate) fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match parse_args(args.into()) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);
    // Read before the struct is emitted: a field's `#[config(…)]` is the
    // decorator's own, so it leaves the item it is read off.
    let fields = match Fields::take(&mut item, args.manual_from_env) {
        Ok(fields) => fields,
        Err(err) => return err.to_compile_error().into(),
    };
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let namespace_lit = args.namespace.value();

    // Without `crate = ` the derive emits `::validator::`, which the call site
    // would have to depend on.
    let derive = (!args.manual_validate).then(|| {
        quote! {
            #[derive(::nest_rs_config::__private::validator::Validate)]
            #[validate(crate = ::nest_rs_config::__private::validator)]
        }
    });

    let from_env = (!args.manual_from_env).then(|| overlay(&item, &fields, args.defaults.as_ref()));

    // A provider's injection is the whole declaration of a config: the boot
    // queues its factory then. A generic config has no one type to file, and
    // is queued by the module reading it.
    let injected = item.generics.params.is_empty().then(|| {
        quote! {
            ::nest_rs_config::__private::inventory::submit! {
                ::nest_rs_config::__private::InjectedFactory::new(
                    || ::core::any::TypeId::of::<#name>(),
                    ::nest_rs_config::__private::queue_feature::<#name>,
                    ::nest_rs_config::__private::check_feature::<#name>,
                )
            }
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
            const __DECLARATION: &'static str = #declaration;
        }

        ::nest_rs_config::__private::inventory::submit! {
            ::nest_rs_config::__private::ConfigNamespace::new(#namespace_lit, #declaration)
        }

        #from_env

        #injected
    }
    .into()
}

struct Args {
    namespace: LitStr,
    manual_validate: bool,
    manual_from_env: bool,
    defaults: Option<syn::Path>,
}

/// Every key `#[config]` takes on its struct, in the order its refusals list
/// them.
const CONFIG: Grammar = Grammar::new("config", &["namespace", "validate", "from_env", "defaults"])
    .elsewhere(a_fields_key);

/// A field's key written on the struct is not misspelled: it is told whose it is.
fn a_fields_key(key: &str) -> Option<String> {
    crate::overlay::FIELD.keys().contains(&key).then(|| {
        format!("#[config] `{key}` is a field's: write `#[config({key} …)]` on the field it reads")
    })
}

/// The stem rule, worded with the rule's own examples: the namespace is read
/// off the file's path, never chosen, so no single value is prescribed.
const MISSING_NAMESPACE: &str = "#[config] requires `namespace`, the stem of the file's path \
     below `src/`: the crate's subject, then each folder, joined with `__`, a plural role \
     folder and a product's container left out — `redis/src/queue/config.rs` reads \
     `namespace = \"redis__queue\"`, `social/src/providers/github/config.rs` \
     `namespace = \"social__github\"`, `features/src/sign_in/config.rs` `namespace = \"sign_in\"`";

fn parse_args(args: TokenStream2) -> syn::Result<Args> {
    let mut namespace: Option<LitStr> = None;
    let mut manual_validate = false;
    let mut manual_from_env = None;
    let mut defaults = None;
    CONFIG.parse2(args, |arg| {
        match arg.key() {
            "namespace" => namespace = Some(arg.str_lit("redis__queue")?),
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
            "from_env" => {
                let lit = arg.str_lit("manual")?;
                if lit.value() != "manual" {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        "#[config] `from_env` takes only `\"manual\"`, which leaves the \
                         struct to write `impl Config` itself, its overlay included",
                    ));
                }
                manual_from_env = Some(lit);
            }
            "defaults" => {
                let value = arg.expr()?;
                let syn::Expr::Path(path) = value else {
                    return Err(syn::Error::new_spanned(
                        &value,
                        "#[config] `defaults` takes the path of a `fn() -> Self`, e.g. \
                         `defaults = Self::for_profile`",
                    ));
                };
                defaults = Some(path.path);
            }
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;

    let lit = namespace
        .ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), MISSING_NAMESPACE))?;
    validate_namespace(&lit)?;
    if let (Some(manual), Some(path)) = (&manual_from_env, &defaults) {
        let mut refused = syn::Error::new_spanned(
            path,
            "#[config] `defaults` is the derived `impl Config`'s: under `from_env = \"manual\"` \
             the struct's own `impl Config` writes `fn defaults()`",
        );
        refused.combine(syn::Error::new_spanned(
            manual,
            "the overlay is written by hand here",
        ));
        return Err(refused);
    }
    Ok(Args {
        namespace: lit,
        manual_validate,
        manual_from_env: manual_from_env.is_some(),
        defaults,
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
