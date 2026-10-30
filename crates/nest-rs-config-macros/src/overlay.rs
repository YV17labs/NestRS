//! The overlay `#[config]` writes: `Config::from_env`, one field at a time, each
//! key the field's name and each reader chosen by the field's type.

use nest_rs_codegen::Grammar;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Expr, Ident, ItemStruct, Path, Type};

/// Every key `#[config]` takes on a field, in the order its refusals list them.
pub(crate) const FIELD: Grammar =
    Grammar::new("config", &["bounds", "json", "count", "with"]).elsewhere(the_structs_key);

/// A struct's key written on a field is not misspelled: it is told whose it is.
fn the_structs_key(key: &str) -> Option<String> {
    ["namespace", "validate", "from_env", "defaults"]
        .contains(&key)
        .then(|| format!("#[config] `{key}` is the struct's: write it on `#[config(…)]` above it"))
}

/// How one field is read.
enum Reader {
    /// By its type, through `ConfigField`.
    Typed,
    /// A `Duration` or an `Option<Duration>` through the `DurationBounds` named.
    Bounds(Expr),
    /// A structured value, as JSON.
    Json,
    /// An `Option<usize>` whose `0` is unlimited.
    Count,
    /// Through the function named, `fn(&ConfigService, &str, T) -> Result<T>`.
    With(Path),
}

/// One field: its ident, its type, its key, and how it is read.
struct Field {
    ident: Ident,
    ty: Type,
    key: String,
    reader: Reader,
}

/// Every field of a `#[config]` struct, as its overlay reads them.
pub(crate) struct Fields(Vec<Field>);

impl Fields {
    /// Read every field's `#[config(…)]` off `item`, leaving the attribute
    /// nowhere a compiler would see it, and refuse what no key can be read
    /// off: a tuple struct, a key holding `__` or naming a unit. Under
    /// `from_env = "manual"` a field takes no key, since nothing reads it.
    pub(crate) fn take(item: &mut ItemStruct, manual: bool) -> syn::Result<Self> {
        let named = match &mut item.fields {
            syn::Fields::Named(named) => Some(named),
            syn::Fields::Unit => None,
            syn::Fields::Unnamed(unnamed) if manual => {
                for field in &unnamed.unnamed {
                    refuse_field_keys(&field.attrs)?;
                }
                return Ok(Self(Vec::new()));
            }
            syn::Fields::Unnamed(unnamed) => {
                return Err(syn::Error::new_spanned(
                    unnamed,
                    "#[config] reads each field under its name, and a tuple struct's fields \
                     have none — name them, or write the overlay with `from_env = \"manual\"`",
                ));
            }
        };
        let mut fields = Vec::new();
        for field in named.into_iter().flat_map(|named| named.named.iter_mut()) {
            let Some(ident) = field.ident.clone() else {
                continue;
            };
            let name = ident.unraw().to_string();
            refuse_unread_key(&ident, &name)?;
            if manual {
                refuse_field_keys(&field.attrs)?;
                continue;
            }
            let reader = take_reader(&mut field.attrs)?;
            fields.push(Field {
                ident,
                ty: field.ty.clone(),
                key: name.to_ascii_uppercase(),
                reader,
            });
        }
        Ok(Self(fields))
    }
}

/// A key the loader would refuse, or one that names a unit a `Duration` carries.
fn refuse_unread_key(ident: &Ident, name: &str) -> syn::Result<()> {
    if name.contains("__") {
        return Err(syn::Error::new_spanned(
            ident,
            format!(
                "#[config] reads the field `{name}` under its name, and a key holds no `__`, \
                 the namespace's separator — rename the field"
            ),
        ));
    }
    if let Some(stem) = name
        .strip_suffix("_secs")
        .or_else(|| name.strip_suffix("_ms"))
    {
        return Err(syn::Error::new_spanned(
            ident,
            format!(
                "#[config] field `{name}` names a unit: a duration is a `Duration` named for \
                 what it bounds, `{stem}`, read through `#[config(bounds = …)]`, whose key \
                 carries the unit"
            ),
        ));
    }
    Ok(())
}

/// Under `from_env = "manual"`, a field's `#[config(…)]` reads nothing: refused.
fn refuse_field_keys(attrs: &[syn::Attribute]) -> syn::Result<()> {
    if let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident("config")) {
        return Err(syn::Error::new_spanned(
            attr,
            "#[config] on a field says how the derived overlay reads it, and under \
             `from_env = \"manual\"` the struct's own `from_env` reads every field",
        ));
    }
    Ok(())
}

/// The field's reader, its `#[config(…)]` removed from `attrs`.
fn take_reader(attrs: &mut Vec<syn::Attribute>) -> syn::Result<Reader> {
    let mut reader = Reader::Typed;
    let mut written: Option<Ident> = None;
    let mut kept = Vec::with_capacity(attrs.len());
    for attr in attrs.drain(..) {
        if !attr.path().is_ident("config") {
            kept.push(attr);
            continue;
        }
        FIELD.parse_attr(&attr, |arg| {
            if let Some(first) = &written {
                return Err(syn::Error::new_spanned(
                    arg.ident(),
                    format!(
                        "#[config] reads a field one way: `{first}` already says how, so \
                         `{}` has nothing left to read",
                        arg.key()
                    ),
                ));
            }
            written = Some(arg.ident().clone());
            reader = match arg.key() {
                "bounds" => Reader::Bounds(arg.expr()?),
                "json" => Reader::Json,
                "count" => Reader::Count,
                "with" => match arg.expr()? {
                    Expr::Path(path) => Reader::With(path.path),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "#[config] `with` takes the path of a \
                             `fn(&ConfigService, &str, T) -> Result<T>`, e.g. \
                             `with = pseudonym_key`",
                        ));
                    }
                },
                // The grammar hands over only its own keys.
                _ => Reader::Typed,
            };
            Ok(())
        })?;
    }
    *attrs = kept;
    Ok(reader)
}

/// `impl Config`, its `from_env` the overlay of every field, and `defaults`
/// when the struct named one; then a compile-time check per bounded field.
pub(crate) fn overlay(item: &ItemStruct, fields: &Fields, defaults: Option<&Path>) -> TokenStream2 {
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    // Mixed-site: a field named `env` or `base` cannot shadow them.
    let env = Ident::new("env", Span::mixed_site());
    let base = Ident::new("base", Span::mixed_site());
    let reads = fields.0.iter().map(|field| {
        let Field {
            ident,
            ty,
            key,
            reader,
        } = field;
        let read = match reader {
            Reader::Typed => quote_spanned! {ty.span()=>
                <#ty as ::nest_rs_config::ConfigField>::overlay(#env, #key, #base.#ident)
            },
            Reader::Bounds(bounds) => quote_spanned! {ty.span()=>
                <#ty as ::nest_rs_config::__private::BoundedField>::overlay_within(
                    &#bounds, #env, #base.#ident,
                )
            },
            Reader::Json => quote! {
                ::nest_rs_config::__private::overlay_json::<#ty>(#env, #key, #base.#ident)
            },
            Reader::Count => quote_spanned! {ty.span()=>
                ::nest_rs_config::ConfigService::count(#env, #key, #base.#ident)
            },
            Reader::With(path) => quote_spanned! {path.span()=>
                #path(#env, #key, #base.#ident)
            },
        };
        quote! { #ident: #read? }
    });
    let checks = fields.0.iter().filter_map(|field| {
        let Reader::Bounds(bounds) = &field.reader else {
            return None;
        };
        let (key, ident) = (&field.key, field.ident.unraw().to_string());
        Some(quote_spanned! {bounds.span()=>
            const _: () = ::core::assert!(
                ::nest_rs_config::__private::reads_field(
                    &#bounds, #key, ::core::stringify!(#name), #ident,
                ),
                "the `DurationBounds` a `#[config(bounds = …)]` names reads the field it \
                 bounds: its key is the field's name in SCREAMING_SNAKE with its unit after \
                 it (`_SECS`, `_MS`), and its field is `Config::field`",
            );
        })
    });
    let body = if fields.0.is_empty() {
        quote! { ::core::result::Result::Ok(#base) }
    } else {
        quote! { ::core::result::Result::Ok(Self { #(#reads,)* }) }
    };
    let defaults = defaults.map(|path| {
        quote_spanned! {path.span()=>
            fn defaults() -> Self {
                #path()
            }
        }
    });
    quote! {
        impl #impl_generics ::nest_rs_config::Config for #name #ty_generics #where_clause {
            fn from_env(
                #env: &::nest_rs_config::ConfigService,
                #base: Self,
            ) -> ::nest_rs_config::Result<Self> {
                #body
            }

            #defaults
        }

        #(#checks)*
    }
}
