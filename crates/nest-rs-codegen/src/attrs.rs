//! Attribute-extraction helpers shared by the transport decorator macros —
//! finding, consuming and validating whole `#[...]` attributes off an item
//! (as opposed to [`crate::args`], which parses the values *inside* one).
//!
//! These gate the Layer-System surface (`#[use_guards]` / `#[force_guards]` /
//! `#[public]` and their HTTP-only siblings), so every transport reads them
//! from one place instead of keeping drifting copies.

use syn::punctuated::Punctuated;
use syn::{Attribute, Path, Token};

/// Extract and remove a flag attribute (no args, no parens) like `#[public]`.
/// `Ok(true)` when present (and removed), `Ok(false)` when absent.
///
/// **An argument on a flag is a compile error, not a discard**, and that is the
/// whole reason this returns a `Result`. [`Attribute::path`] answers the same
/// for `#[public]`, `#[public(admin)]` and `#[public = "x"]`, so a `position` +
/// `remove` on the path alone accepted all three and dropped what the developer
/// wrote — *"never an ignored argument"* (`CLAUDE.md`, *One declaration, every
/// site the standard permits*). The doc above this function said "no args, no
/// parens" while the body enforced nothing, which is the drift the sentence
/// closes.
///
/// **`#[public]` is why it ranks where it does.** It is the posture
/// declaration — one of the three greppable sites `CLAUDE.md` reserves for the
/// authn/authz decision — and it sits beside `#[authorize(Action, Entity)]`,
/// which *does* take arguments. A developer writing `#[public(read_only)]` by
/// analogy shipped an ungated, unmasked operation with the compiler silent.
/// The other five flags this covers (`on_connect`, `on_disconnect`, `no_pipes`,
/// `crud_write`, `crud_location`) get the refusal for free, which is what
/// *"refusals are shared, not per key"* buys.
pub fn take_flag_attr(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<bool> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(false);
    };
    let attr = attrs.remove(pos);
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

/// Extract and remove a `#[<ident>(PathA, PathB)]` attribute, returning its
/// comma-separated paths (empty when absent). The attribute is consumed so it
/// never reaches the compiler as unknown. At most one is accepted; a second of
/// the same ident is rejected with a clear message.
///
/// **The noun is derived from the attribute, not passed in.** It was a
/// parameter, and the same `#[use_guards]` said "list every **entry** in it" on
/// a controller and "list every **guard** in it" on a resolver — one attribute,
/// one rule, two sentences split by edge, which is exactly what *One
/// declaration, every site the standard permits* exists to remove ("same key,
/// same grammar, one shared parser… so learning it once is learning it
/// everywhere"). `"entry"` was a placeholder for a noun rather than a noun, and
/// it was passed at eleven HTTP sites and three WS ones for elements that
/// demonstrably were guards, filters and pipes.
pub fn take_path_list(attrs: &mut Vec<Attribute>, ident: &str) -> syn::Result<Vec<Path>> {
    let noun = &listed_noun(ident);
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident(ident)) else {
        return Ok(Vec::new());
    };
    let attr = attrs.remove(pos);
    if attrs.iter().any(|a| a.path().is_ident(ident)) {
        return Err(syn::Error::new_spanned(
            &attr,
            format!("at most one `#[{ident}(...)]` is allowed; list every {noun} in it"),
        ));
    }
    Ok(attr
        .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

/// What `#[use_guards]` and its siblings list, one word, derived from the
/// attribute's own name: `use_guards` / `force_guards` ⇒ `guard`,
/// `use_filters` ⇒ `filter`, `use_interceptors` ⇒ `interceptor`.
///
/// A fallback of `entry` for a name that fits no pattern, which is what every
/// call site used to pass by hand — it stays as the *default* rather than the
/// answer, so a new family reads as unremarkable until someone names it.
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

/// Every layer family whose binding attribute is **HTTP-only**.
///
/// Four, not two, and the two that were missing are the reason this is a
/// constant rather than an inline array: `#[use_pipes]` and
/// `#[use_exception_filters]` are taken by `#[controller]` / `#[routes]` and
/// nowhere else, exactly as their two neighbours are, so writing either on a
/// gateway, a resolver or an `#[mcp]` host reached rustc as
/// `cannot find attribute … in this scope` — no transport named, no reason, no
/// remedy. `framework.md` item 8 asks for "a named compile error for **every**
/// layer family the edge does not bridge"; the list is what makes "every"
/// checkable.
///
/// Guards are bridged at all four edges and are deliberately absent.
const HTTP_ONLY_LAYERS: [&str; 4] = [
    "use_interceptors",
    "use_filters",
    "use_pipes",
    "use_exception_filters",
];

/// Reject the [`HTTP_ONLY_LAYERS`] binding attributes where they are HTTP-only
/// today: on transports with no per-message/per-operation seam for those traits,
/// binding one would be a silent no-op, so it is a named compile error instead.
/// Guards *are* bridged everywhere, so they stay.
///
/// `transport` (e.g. `"WebSockets"`, `"GraphQL"`) and `site` (e.g. `"gateway"`,
/// `"resolver"`) name the rejecting context in the diagnostic. The sentence
/// says "on this {site}" rather than "on a {site}": several callers pass
/// `"operation"`, and an article baked into the template cannot be right for
/// every noun a future edge will pass. **Pass the site the compiler is
/// underlining**, not the host it belongs to: an attribute on a
/// `#[subscribe_message]` method reported "on this gateway" told the reader to
/// look at the wrong item.
pub fn reject_http_only_layers(
    attrs: &[Attribute],
    transport: &str,
    site: &str,
) -> syn::Result<()> {
    for attr in attrs {
        for name in HTTP_ONLY_LAYERS {
            if attr.path().is_ident(name) {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!(
                        "`#[{name}]` is not bridged on {transport} yet — it would be a silent \
                         no-op on this {site}. Remove it, or move the layer onto an HTTP \
                         `#[controller]` / `#[routes]`, where the pooled layer families run. \
                         `#[use_guards]` is bridged at every edge and works here.",
                    ),
                ));
            }
        }
    }
    Ok(())
}
