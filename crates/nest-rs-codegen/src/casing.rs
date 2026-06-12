//! Identifier casing helpers shared by decorator macros.

use syn::Ident;

/// `AudioJobs` → `audio_jobs`. Camel/Pascal → snake, inserting `_` before each
/// interior uppercase. Shared by `#[processor]`/`#[scheduled]`-style macros that
/// derive a stable wire/queue name from a struct ident.
pub fn snake_case(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, ch) in camel.chars().enumerate() {
        if ch.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// `org_id` → `OrgId`. Matches SeaORM's `Column` enum naming and the
/// `<Service>By<Method>` loader struct convention from `#[dataloader]`.
pub fn pascal_case(ident: &Ident) -> Ident {
    let mut out = String::new();
    let mut upper = true;
    for ch in ident.to_string().chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    Ident::new(&out, ident.span())
}
