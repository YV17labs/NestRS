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

/// What of a decorated method's attributes — or of one of its parameters' —
/// describes the field an expansion writes beside it, to the attribute macro
/// that reads that field: `helper`, that macro's own attribute; its docs; its
/// `#[cfg]`s, each one inside a `#[cfg_attr]` unfolded into a plain `#[cfg]`;
/// and the lints it allows, an `#[expect]` as an `#[allow]` of the same lints
/// and reason. Everything else is the method's alone, and is not here.
///
/// async-graphql's `#[Object]` and `#[ComplexObject]` read a plain `#[cfg]`
/// alone. `#[cfg_attr(p, cfg(x))]` unfolds to `cfg(any(not(p), x))`, not
/// `cfg(all(p, x))`. The field trips some of the lints the method expects and
/// not others, so an `#[expect]` is checked on the method alone. A `helper`
/// inside a `#[cfg_attr]` is refused: the macro reading it expands before any
/// condition is evaluated.
pub fn field_attrs(attrs: &[Attribute], helper: &str) -> syn::Result<Vec<Attribute>> {
    let mut described = Vec::new();
    for attr in attrs {
        let path = attr.path();
        if path.is_ident("doc")
            || path.is_ident("cfg")
            || path.is_ident("allow")
            || path.is_ident(helper)
        {
            described.push(attr.clone());
        } else if path.is_ident("expect")
            && let syn::Meta::List(list) = &attr.meta
        {
            let lints = &list.tokens;
            described.push(syn::parse_quote!(#[allow(#lints)]));
        } else if path.is_ident("cfg_attr")
            && let syn::Meta::List(list) = &attr.meta
        {
            let mut unfolded = Vec::new();
            unfold_cfg_attr(&[], list.tokens.clone(), helper, &mut unfolded)?;
            for tokens in unfolded {
                described.extend(syn::parse::Parser::parse2(Attribute::parse_outer, tokens)?);
            }
        }
    }
    Ok(described)
}

/// What the entries of one `cfg_attr(predicate, ..)` describe of a field
/// ([`field_attrs`]), under every enclosing predicate in `outer`, pushed onto
/// `into`. Arguments that are not the attribute's grammar describe nothing:
/// rustc refuses them on the method, which keeps them.
fn unfold_cfg_attr(
    outer: &[TokenStream],
    arguments: TokenStream,
    helper: &str,
    into: &mut Vec<TokenStream>,
) -> syn::Result<()> {
    let mut entries = split_at_commas(arguments).into_iter();
    let Some(predicate) = entries.next().filter(|predicate| !predicate.is_empty()) else {
        return Ok(());
    };
    let mut predicates = outer.to_vec();
    predicates.push(predicate);
    let conjunction = quote!(all(#(#predicates),*));
    for entry in entries {
        let entry = ungrouped_tokens(entry);
        let mut tokens = entry.clone().into_iter();
        let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(group)), None) =
            (tokens.next(), tokens.next(), tokens.next())
        else {
            continue;
        };
        if group.delimiter() != Delimiter::Parenthesis {
            continue;
        }
        let inner = group.stream();
        if name == "cfg" {
            into.push(quote!(#[cfg(any(not(#conjunction), #inner))]));
        } else if name == "cfg_attr" {
            unfold_cfg_attr(&predicates, inner, helper, into)?;
        } else if name == "expect" || name == "allow" {
            into.push(quote!(#[cfg_attr(#conjunction, allow(#inner))]));
        } else if name == helper {
            return Err(syn::Error::new_spanned(
                &entry,
                format!(
                    "`#[{helper}(…)]` inside a `#[cfg_attr]` would be read by nothing: the macro \
                     reading it expands before any condition is evaluated — write it on a line \
                     of its own, and put the condition on the method as a `#[cfg]`"
                ),
            ));
        }
    }
    Ok(())
}

/// Whether an item is written `#[name]`, plainly or as an entry of a
/// `#[cfg_attr]` at any depth, whatever its arguments.
pub fn is_written(attrs: &[Attribute], name: &str) -> bool {
    fn among(arguments: TokenStream, name: &str) -> bool {
        split_at_commas(arguments).into_iter().skip(1).any(|entry| {
            let mut tokens = ungrouped_tokens(entry).into_iter();
            match (tokens.next(), tokens.next()) {
                (Some(TokenTree::Ident(written)), Some(TokenTree::Group(group)))
                    if written == "cfg_attr" =>
                {
                    among(group.stream(), name)
                }
                (Some(TokenTree::Ident(written)), next) => {
                    written == name
                        && !matches!(next, Some(TokenTree::Punct(punct)) if punct.as_char() == ':')
                }
                _ => false,
            }
        })
    }
    attrs.iter().any(|attr| {
        attr.path().is_ident(name)
            || (attr.path().is_ident("cfg_attr")
                && matches!(&attr.meta, syn::Meta::List(list) if among(list.tokens.clone(), name)))
    })
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
pub(crate) fn split_at_commas(tokens: TokenStream) -> Vec<TokenStream> {
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

/// Extract and remove a flag attribute (no args, no parens) like `#[no_pipes]`.
/// `Ok(true)` when present (and removed), `Ok(false)` when absent.
///
/// An argument on a flag is a compile error, never a discard: `#[no_pipes(x)]`
/// must not silently run no pipe.
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
                .map_or_default(|first| first.to_uppercase().chain(letters).collect::<String>())
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

    fn described(method: &syn::ImplItemFn) -> Vec<String> {
        field_attrs(&method.attrs, "graphql")
            .expect("described")
            .iter()
            .map(|attr| quote!(#attr).to_string())
            .collect()
    }

    #[test]
    fn a_cfg_inside_a_cfg_attr_is_unfolded_for_a_field() {
        let method: syn::ImplItemFn = parse_quote! {
            #[doc = "a method"]
            #[cfg(feature = "x")]
            #[cfg_attr(test, cfg(unix), must_use)]
            #[cfg_attr(a, cfg_attr(b, cfg(c)))]
            fn run(&self) {}
        };
        assert_eq!(
            described(&method),
            [
                quote!(#[doc = "a method"]).to_string(),
                quote!(#[cfg(feature = "x")]).to_string(),
                quote!(#[cfg(any(not(all(test)), unix))]).to_string(),
                quote!(#[cfg(any(not(all(a, b)), c))]).to_string(),
            ]
        );
    }

    /// A field carries what the macro reading it reads and the allowances its
    /// repeated signature needs: every other attribute is the method's alone,
    /// whatever its name, plain or under a condition.
    #[test]
    fn a_field_is_described_by_its_helper_docs_conditions_and_allowances_alone() {
        let method: syn::ImplItemFn = parse_quote! {
            /// Read by the schema.
            #[graphql(name = "run", complexity = 5)]
            #[tracing::instrument(skip(self), err)]
            #[resilient(upstream, idempotent)]
            #[shielded(upstream)]
            #[must_use]
            #[deprecated = "use `walk`"]
            #[inline]
            #[deny(unused_must_use)]
            #[warn(clippy::pedantic)]
            #[forbid(unsafe_code)]
            #[allow(unused_mut)]
            #[cfg_attr(test, tracing::instrument(skip_all), resilient(upstream), deny(dead_code))]
            #[cfg_attr(test, doc = "under test", allow(unused))]
            #[cfg_attr]
            #[cfg_attr()]
            #[cfg_attr(, must_use)]
            fn run(&self) {}
        };
        assert_eq!(
            described(&method),
            [
                quote!(#[doc = r" Read by the schema."]).to_string(),
                quote!(#[graphql(name = "run", complexity = 5)]).to_string(),
                quote!(#[allow(unused_mut)]).to_string(),
                quote!(#[cfg_attr(all(test), allow(unused))]).to_string(),
            ]
        );
    }

    /// An expectation is checked on the developer's method alone: its field
    /// allows the same lints for the same reason, plain or under a condition.
    #[test]
    fn an_expectation_reaches_a_field_as_an_allowance() {
        let method: syn::ImplItemFn = parse_quote! {
            #[expect(unused_variables, reason = "read next release")]
            #[cfg_attr(test, expect(clippy::too_many_arguments))]
            #[cfg_attr(a, cfg_attr(b, expect(dead_code), cfg(c)))]
            #[allow(unused_mut)]
            fn run(&self, unread: u32) {}
        };
        assert_eq!(
            described(&method),
            [
                quote!(#[allow(unused_variables, reason = "read next release")]).to_string(),
                quote!(#[cfg_attr(all(test), allow(clippy::too_many_arguments))]).to_string(),
                quote!(#[cfg_attr(all(a, b), allow(dead_code))]).to_string(),
                quote!(#[cfg(any(not(all(a, b)), c))]).to_string(),
                quote!(#[allow(unused_mut)]).to_string(),
            ]
        );
    }

    #[test]
    fn a_helper_inside_a_cfg_attr_is_refused_at_its_entry() {
        for method in [
            quote! {
                #[cfg_attr(feature = "federated", graphql(shareable))]
                fn run(&self) {}
            },
            quote! {
                #[cfg_attr(a, must_use, cfg_attr(b, graphql(name = "run")))]
                fn run(&self) {}
            },
        ] {
            let method: syn::ImplItemFn = syn::parse2(method).expect("a method");
            let refused = field_attrs(&method.attrs, "graphql")
                .map(|_| ())
                .expect_err("refused")
                .to_string();
            assert!(
                refused
                    .starts_with("`#[graphql(…)]` inside a `#[cfg_attr]` would be read by nothing"),
                "{refused}"
            );
        }
    }

    #[test]
    fn an_entry_passed_through_a_macro_fragment_describes_the_field() {
        let fragment = proc_macro2::Group::new(Delimiter::None, quote!(expect(dead_code)));
        let method: syn::ImplItemFn = parse_quote! {
            #[cfg_attr(all(), #fragment, must_use)]
            fn run(&self) {}
        };
        assert_eq!(
            described(&method),
            [quote!(#[cfg_attr(all(all()), allow(dead_code))]).to_string()]
        );
    }

    #[test]
    fn an_attribute_is_found_written_plainly_or_under_any_condition() {
        let written = |method: syn::ImplItemFn| is_written(&method.attrs, "deprecated");
        assert!(written(parse_quote! {
            #[deprecated]
            fn run(&self) {}
        }));
        assert!(written(parse_quote! {
            #[deprecated(since = "2.0.0", note = "use `walk`")]
            fn run(&self) {}
        }));
        assert!(written(parse_quote! {
            #[cfg_attr(feature = "next", must_use, deprecated = "use `walk`")]
            fn run(&self) {}
        }));
        assert!(written(parse_quote! {
            #[cfg_attr(a, cfg_attr(b, deprecated))]
            fn run(&self) {}
        }));
        assert!(!written(parse_quote! {
            #[cfg_attr(deprecated, must_use)]
            #[doc = "deprecated"]
            #[deprecated::audit(run)]
            #[cfg_attr(a, deprecated::audit(run), allow(deprecated))]
            fn run(&self) {}
        }));
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
