//! Identifier casing helpers shared by decorator macros.

use syn::Ident;

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
