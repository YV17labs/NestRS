//! The overlay `#[config]` writes: `Config::from_env`, one field at a time, each
//! key the field's name and each reader chosen by the field's type — and, per
//! bounded field, the `BoundedSetting` it reads through, named for the field.

use nest_rs_codegen::Grammar;
use proc_macro2::{Group, Span, TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Expr, Ident, ItemStruct, LitStr, Path, Type, Visibility};

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
    /// Through the range named, bound to the field: a `Duration` or an
    /// `Option<Duration>` held to a `DurationBounds`, a `usize` to a
    /// `CountBounds`.
    Bounds(Expr),
    /// A structured value, as JSON.
    Json,
    /// An `Option<usize>` whose `0` is unlimited.
    Count,
    /// Through the function named, `fn(&ConfigService, Field, T) -> Result<T>`.
    With(Path),
}

/// One field: its ident, its visibility, its type, its key, and how it is read.
struct Field {
    ident: Ident,
    vis: Visibility,
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
                vis: field.vis.clone(),
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
                             `fn(&ConfigService, Field, T) -> Result<T>`, e.g. \
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
/// when the struct named one; then, per bounded field, the setting it reads
/// through, an associated constant named for the field.
pub(crate) fn overlay(item: &ItemStruct, fields: &Fields, defaults: Option<&Path>) -> TokenStream2 {
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    // Mixed-site: a field named `env` or `base` cannot shadow them.
    let env = Ident::new("env", Span::mixed_site());
    let base = Ident::new("base", Span::mixed_site());
    let field_of = |field: &Field| {
        let path = LitStr::new(
            &format!("{}::{}", name.unraw(), field.ident.unraw()),
            field.ident.span(),
        );
        let key = &field.key;
        quote! { ::nest_rs_config::Field::new(#key, #path) }
    };
    let reads = fields.0.iter().map(|field| {
        let Field {
            ident,
            ty,
            key,
            reader,
            ..
        } = field;
        let read = match reader {
            Reader::Typed => quote_spanned! {ty.span()=>
                <#ty as ::nest_rs_config::ConfigField>::overlay(#env, #key, #base.#ident)
            },
            Reader::Bounds(_) => {
                let at = ty.span();
                let setting = Ident::new(&field.key, at);
                let (env, base, ident) =
                    (located(&env, at), located(&base, at), located(ident, at));
                quote_spanned! {at=>
                    <#ty as ::nest_rs_config::BoundedField>::read(
                        &Self::#setting, #env, #base.#ident,
                    )
                }
            }
            Reader::Json => quote! {
                ::nest_rs_config::__private::overlay_json::<#ty>(#env, #key, #base.#ident)
            },
            Reader::Count => quote_spanned! {ty.span()=>
                ::nest_rs_config::ConfigService::count(#env, #key, #base.#ident)
            },
            Reader::With(path) => {
                let field = field_of(field);
                quote_spanned! {path.span()=>
                    #path(#env, #field, #base.#ident)
                }
            }
        };
        quote! { #ident: #read? }
    });
    let settings: Vec<TokenStream2> = fields
        .0
        .iter()
        .filter_map(|field| {
            let Reader::Bounds(bounds) = &field.reader else {
                return None;
            };
            let (vis, ty, setting) = (&field.vis, &field.ty, setting_ident(field));
            let doc = format!(
                "`{}`'s range, bound to the variable it is read from and the field that pins \
                 it: a `BoundedSetting` over the field's type.",
                field.ident.unraw()
            );
            let field = relocated(field_of(field), ty.span());
            // Every token naming the field's type goes through `BoundedField`
            // at that type's location, so rustc says a type no range holds, or
            // a range of the other kind, once — at the field.
            let at = ty.span();
            let range = Ident::new("__NESTRS_RANGE", at);
            let bounds = relocated(bounds.to_token_stream(), at);
            Some(quote_spanned! {at=>
                #[doc = #doc]
                #vis const #setting: <#ty as ::nest_rs_config::BoundedField>::Setting = {
                    const #range: <#ty as ::nest_rs_config::BoundedField>::Bounds = #bounds;
                    ::nest_rs_config::BoundedSetting::new(#range, #field)
                };
            })
        })
        .collect();
    let settings = (!settings.is_empty()).then(|| {
        quote! {
            impl #impl_generics #name #ty_generics #where_clause {
                #(#settings)*
            }
        }
    });
    // Read whole, a field's variable sets everything a refusal finds inside it.
    let whole: Vec<String> = fields
        .0
        .iter()
        .filter(|field| matches!(field.reader, Reader::Json))
        .map(|field| field.ident.unraw().to_string())
        .collect();
    let whole = (!whole.is_empty()).then(|| {
        quote! {
            const __WHOLE_FIELDS: &'static [&'static str] = &[#(#whole),*];
        }
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

            #whole

            #defaults
        }

        #settings
    }
}

/// The associated constant a bounded field's setting is bound to: the field's
/// name in SCREAMING_SNAKE, its key's stem, at the field type's location.
fn setting_ident(field: &Field) -> Ident {
    Ident::new(&field.key, field.ty.span())
}

/// `ident`, its hygiene kept, at `at`'s location.
fn located(ident: &Ident, at: Span) -> Ident {
    let mut ident = ident.clone();
    ident.set_span(ident.span().located_at(at));
    ident
}

/// `tokens`, each keeping its hygiene, at `at`'s location.
fn relocated(tokens: TokenStream2, at: Span) -> TokenStream2 {
    tokens
        .into_iter()
        .map(|mut tree| {
            if let TokenTree::Group(group) = &tree {
                let mut moved = Group::new(group.delimiter(), relocated(group.stream(), at));
                moved.set_span(group.span().located_at(at));
                return TokenTree::Group(moved);
            }
            tree.set_span(tree.span().located_at(at));
            tree
        })
        .collect()
}
