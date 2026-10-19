//! Attribute-extraction helpers shared by the transport decorator macros —
//! finding, consuming and validating whole `#[...]` attributes off an item
//! (as opposed to [`crate::args`], which parses the values *inside* one).

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::ungrouped::ungrouped_tokens;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Attribute, Path, Token};

/// The `#[cfg]` conditions of a decorated method — written plainly, or inside a
/// `#[cfg_attr]` — for every item its expansion emits beside the method.
///
/// An attribute macro on an `impl` block receives its methods before any
/// `#[cfg]` is evaluated, so an item emitted for a compiled-out method without
/// the same condition names a method that does not exist. A `#[cfg_attr]` is
/// forwarded holding its `cfg(..)`s alone.
pub fn cfg_attrs(attrs: &[Attribute]) -> Vec<TokenStream> {
    attrs
        .iter()
        .filter_map(|attr| {
            if attr.path().is_ident("cfg") {
                return Some(quote!(#attr));
            }
            if !attr.path().is_ident("cfg_attr") {
                return None;
            }
            let conditions = cfg_attr_conditions(attr.meta.require_list().ok()?.tokens.clone())?;
            Some(quote!(#[#conditions]))
        })
        .collect()
}

/// A decorated method's attributes as another attribute macro can read them —
/// every `#[cfg]` inside a `#[cfg_attr]` unfolded into a plain `#[cfg]`, and
/// everything else as written.
///
/// async-graphql's `#[Object]` and `#[ComplexObject]` read a plain `#[cfg]`
/// alone. `#[cfg_attr(p, cfg(x))]` unfolds to `cfg(any(not(p), x))`, not
/// `cfg(all(p, x))`.
pub fn delegated_attrs(attrs: &[Attribute]) -> Vec<Attribute> {
    let mut delegated = Vec::with_capacity(attrs.len());
    for attr in attrs {
        let unfolded = attr
            .path()
            .is_ident("cfg_attr")
            .then(|| attr.meta.require_list().ok())
            .flatten()
            .and_then(|list| {
                let mut tokens = Vec::new();
                unfold_cfg_attr(&[], list.tokens.clone(), &mut tokens).then_some(tokens)
            })
            .and_then(|tokens| {
                tokens
                    .into_iter()
                    .map(|tokens| {
                        syn::parse::Parser::parse2(Attribute::parse_outer, tokens)
                            .ok()
                            .and_then(|mut parsed| parsed.pop())
                    })
                    .collect::<Option<Vec<_>>>()
            });
        match unfolded {
            Some(unfolded) => delegated.extend(unfolded),
            None => delegated.push(attr.clone()),
        }
    }
    delegated
}

/// The attributes one `cfg_attr(predicate, ..)` holds, under every enclosing
/// predicate in `outer`, pushed onto `into` — `false` when the arguments are not
/// the attribute's grammar, so the caller keeps it as written.
fn unfold_cfg_attr(
    outer: &[TokenStream],
    arguments: TokenStream,
    into: &mut Vec<TokenStream>,
) -> bool {
    let mut entries = split_at_commas(arguments).into_iter();
    let Some(predicate) = entries.next().filter(|predicate| !predicate.is_empty()) else {
        return false;
    };
    let mut predicates = outer.to_vec();
    predicates.push(predicate);
    let conjunction = quote!(all(#(#predicates),*));
    for entry in entries {
        let entry = ungrouped_tokens(entry);
        if entry.is_empty() {
            continue;
        }
        let mut tokens = entry.clone().into_iter();
        match (tokens.next(), tokens.next(), tokens.next()) {
            (Some(TokenTree::Ident(name)), Some(TokenTree::Group(group)), None)
                if name == "cfg" && group.delimiter() == Delimiter::Parenthesis =>
            {
                let condition = group.stream();
                into.push(quote!(#[cfg(any(not(#conjunction), #condition))]));
            }
            (Some(TokenTree::Ident(name)), Some(TokenTree::Group(group)), None)
                if name == "cfg_attr" && group.delimiter() == Delimiter::Parenthesis =>
            {
                if !unfold_cfg_attr(&predicates, group.stream(), into) {
                    return false;
                }
            }
            _ => into.push(quote!(#[cfg_attr(#conjunction, #entry)])),
        }
    }
    true
}

/// An item a decorated method declares — a guard it binds, a dependency it
/// resolves — under the `#[cfg]` conditions of that method ([`cfg_attrs`]), for a
/// helper that emits something per item *outside* the method.
///
/// A plain `&T` converts with no conditions.
pub struct Conditional<'a, T> {
    /// The method's conditions, empty when it has none.
    pub cfgs: &'a [TokenStream],
    /// The item it declares.
    pub item: &'a T,
}

impl<'a, T> From<&'a T> for Conditional<'a, T> {
    fn from(item: &'a T) -> Self {
        Self { cfgs: &[], item }
    }
}

/// `cfg_attr(predicate, ..)` holding only the `cfg(..)`s among the attributes in
/// `arguments` — a nested `cfg_attr` kept for the `cfg(..)`s inside it — or `None`
/// when it holds none.
///
/// Read as tokens, never as `Meta`: `true` in `cfg_attr(true, ..)` is no path,
/// and a `Meta` parse drops the whole attribute.
pub(crate) fn cfg_attr_conditions(arguments: TokenStream) -> Option<TokenStream> {
    let mut entries = split_at_commas(arguments).into_iter();
    let predicate = entries.next().filter(|predicate| !predicate.is_empty())?;
    let conditions: Vec<TokenStream> = entries
        .filter_map(|entry| {
            let entry = ungrouped_tokens(entry);
            let mut tokens = entry.clone().into_iter();
            let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(group)), None) =
                (tokens.next(), tokens.next(), tokens.next())
            else {
                return None;
            };
            if group.delimiter() != Delimiter::Parenthesis {
                return None;
            }
            if name == "cfg" {
                Some(entry)
            } else if name == "cfg_attr" {
                cfg_attr_conditions(group.stream())
            } else {
                None
            }
        })
        .collect();
    (!conditions.is_empty()).then(|| quote!(cfg_attr(#predicate, #(#conditions),*)))
}

/// `tokens` split at each comma outside a group.
fn split_at_commas(tokens: TokenStream) -> Vec<TokenStream> {
    let mut entries = vec![TokenStream::new()];
    for token in tokens {
        if matches!(&token, TokenTree::Punct(punct) if punct.as_char() == ',') {
            entries.push(TokenStream::new());
        } else if let Some(entry) = entries.last_mut() {
            entry.extend([token]);
        }
    }
    entries
}

/// Extract and remove a flag attribute (no args, no parens) like `#[public]`.
/// `Ok(true)` when present (and removed), `Ok(false)` when absent.
///
/// An argument on a flag is a compile error, never a discard: `#[public(x)]`
/// must not silently declare a public posture.
///
/// ```
/// use nest_rs_codegen::take_flag_attr;
///
/// let mut method: syn::ImplItemFn = syn::parse_quote! {
///     #[audited]
///     fn close(&self) {}
/// };
/// assert!(take_flag_attr(&mut method.attrs, "audited")?);
/// assert!(method.attrs.is_empty());
///
/// let mut method: syn::ImplItemFn = syn::parse_quote! {
///     #[audited(always)]
///     fn close(&self) {}
/// };
/// assert!(take_flag_attr(&mut method.attrs, "audited").is_err());
/// # Ok::<(), syn::Error>(())
/// ```
pub fn take_flag_attr(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<bool> {
    let Some(attr) = take_single_attr(attrs, ident)? else {
        return Ok(false);
    };
    if !matches!(attr.meta, syn::Meta::Path(_)) {
        return Err(syn::Error::new_spanned(
            &attr,
            format!(
                "`#[{ident}]` takes no arguments — it is a flag, and what it declares is \
                 its presence"
            ),
        ));
    }
    Ok(true)
}

/// Remove the one `#[<ident>]` on an item, refusing a second copy by name.
///
/// The span is the second copy, the one to delete.
///
/// ```
/// use nest_rs_codegen::{repeated_attribute, take_single_attr};
///
/// let mut item: syn::ItemStruct = syn::parse_quote! {
///     #[audited(topic = "billing")]
///     struct Invoice;
/// };
/// let taken = take_single_attr(&mut item.attrs, "audited")?;
/// assert!(taken.is_some() && item.attrs.is_empty());
///
/// let mut item: syn::ItemStruct = syn::parse_quote! {
///     #[audited(topic = "billing")]
///     #[audited(topic = "orders")]
///     struct Invoice;
/// };
/// let refused = take_single_attr(&mut item.attrs, "audited").err();
/// assert_eq!(refused.map(|e| e.to_string()), Some(repeated_attribute("audited")));
/// # Ok::<(), syn::Error>(())
/// ```
pub fn take_single_attr(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<Option<Attribute>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(None);
    };
    let attr = attrs.remove(pos);
    if let Some(second) = attrs.iter().find(|a| a.path().is_ident(ident)) {
        return Err(syn::Error::new_spanned(second, repeated_attribute(ident)));
    }
    Ok(Some(attr))
}

/// The sentence [`take_single_attr`] refuses a second copy with, for a
/// decorator that finds the copy itself.
///
/// ```
/// let sentence = nest_rs_codegen::repeated_attribute("audited");
/// assert!(sentence.starts_with("`#[audited]` is written twice here"));
/// ```
pub fn repeated_attribute(ident: &str) -> String {
    format!(
        "`#[{ident}]` is written twice here — it declares once, and a second copy would be \
         read by nothing. Keep one, with everything it says"
    )
}

/// Extract and remove a `#[<ident>(PathA, PathB)]` attribute, returning its
/// comma-separated paths (empty when absent). At most one is accepted; an entry
/// that is not a type path is refused naming the attribute.
///
/// ```
/// use nest_rs_codegen::take_path_list;
///
/// let mut item: syn::ItemStruct = syn::parse_quote! {
///     #[use_auditors(Ledger, crate::audit::Mailer)]
///     struct Invoice;
/// };
/// let listed = take_path_list(&mut item.attrs, "use_auditors")?;
/// assert_eq!(listed.len(), 2);
/// assert!(item.attrs.is_empty());
///
/// let mut item: syn::ItemStruct = syn::parse_quote! {
///     #[use_auditors("ledger")]
///     struct Invoice;
/// };
/// let refused = take_path_list(&mut item.attrs, "use_auditors").err();
/// assert!(refused.is_some_and(|e| e.to_string().starts_with("#[use_auditors] takes a list")));
/// # Ok::<(), syn::Error>(())
/// ```
pub fn take_path_list(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<Vec<Path>> {
    let noun = listed_noun(ident);
    let spoken = noun.replace('_', " ");
    let cased: String = noun
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default()
        })
        .collect();
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident(ident)) {
        return Err(syn::Error::new_spanned(
            &attr,
            format!("at most one `#[{ident}(...)]` is allowed; list every {spoken} in it"),
        ));
    }
    let listed = attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
        .map_err(|stopped| {
            syn::Error::new(
                stopped.span(),
                crate::args::takes_value(
                    ident,
                    None,
                    &format!("a list of {spoken} types, e.g. `#[{ident}(My{cased})]`"),
                ),
            )
        })?;
    Ok(listed.into_iter().collect())
}

/// What `#[use_guards]` and its siblings list, derived from the attribute's
/// name (`use_exception_filters` ⇒ `exception_filter`); `entry` otherwise.
fn listed_noun(ident: &str) -> String {
    let stem = ident
        .strip_prefix("use_")
        .or_else(|| ident.strip_prefix("force_"))
        .unwrap_or(ident);
    match stem.strip_suffix('s') {
        Some(singular) if !singular.is_empty() => singular.to_owned(),
        _ => "entry".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn only_the_conditions_travel() {
        let method: syn::ImplItemFn = parse_quote! {
            #[doc = "a method"]
            #[cfg(feature = "x")]
            #[allow(dead_code)]
            #[cfg_attr(all(), cfg(any()), must_use)]
            #[cfg_attr(test, cfg_attr(unix, cfg(not(miri))))]
            #[cfg_attr(test, allow(unused))]
            #[cfg_attr(true, cfg(false))]
            #[cfg_attr(unix, cfg(a), )]
            async fn run(&self) {}
        };
        let forwarded: Vec<String> = cfg_attrs(&method.attrs)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            forwarded,
            [
                quote!(#[cfg(feature = "x")]).to_string(),
                quote!(#[cfg_attr(all(), cfg(any()))]).to_string(),
                quote!(#[cfg_attr(test, cfg_attr(unix, cfg(not(miri))))]).to_string(),
                quote!(#[cfg_attr(true, cfg(false))]).to_string(),
                quote!(#[cfg_attr(unix, cfg(a))]).to_string(),
            ]
        );
    }

    #[test]
    fn a_cfg_inside_a_cfg_attr_is_unfolded_for_a_delegate() {
        let method: syn::ImplItemFn = parse_quote! {
            #[doc = "a method"]
            #[cfg(feature = "x")]
            #[cfg_attr(test, cfg(unix), must_use)]
            #[cfg_attr(a, cfg_attr(b, cfg(c)))]
            fn run(&self) {}
        };
        let delegated: Vec<String> = delegated_attrs(&method.attrs)
            .iter()
            .map(|attr| quote!(#attr).to_string())
            .collect();
        assert_eq!(
            delegated,
            [
                quote!(#[doc = "a method"]).to_string(),
                quote!(#[cfg(feature = "x")]).to_string(),
                quote!(#[cfg(any(not(all(test)), unix))]).to_string(),
                quote!(#[cfg_attr(all(test), must_use)]).to_string(),
                quote!(#[cfg(any(not(all(a, b)), c))]).to_string(),
            ]
        );
    }

    #[test]
    fn a_condition_passed_through_a_macro_fragment_travels() {
        let fragment = proc_macro2::Group::new(Delimiter::None, quote!(cfg(any())));
        assert_eq!(
            cfg_attr_conditions(quote!(all(), #fragment)).map(|tokens| tokens.to_string()),
            Some(quote!(cfg_attr(all(), cfg(any()))).to_string()),
        );
    }
}
