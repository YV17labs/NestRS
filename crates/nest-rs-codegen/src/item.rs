//! The item a decorator was written on, as its refusal names it.

use syn::Item;

/// The shape of `item` as a refusal names it — `a struct`, `an enum`, `an impl
/// block` — or `None` for one no sentence names, which the decorator words by
/// what it takes (`not a struct`).
pub fn item_shape(item: &Item) -> Option<&'static str> {
    match item {
        Item::Struct(_) => Some("a struct"),
        Item::Enum(_) => Some("an enum"),
        Item::Union(_) => Some("a union"),
        Item::Type(_) => Some("a type alias"),
        Item::Trait(_) => Some("a trait"),
        Item::Fn(_) => Some("a function"),
        Item::Impl(_) => Some("an impl block"),
        Item::Mod(_) => Some("a module"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn a_shape_is_named_as_a_reader_says_it() {
        let named: [(Item, Option<&str>); 4] = [
            (parse_quote! { enum Wire { A } }, Some("an enum")),
            (parse_quote! { impl Wire {} }, Some("an impl block")),
            (parse_quote! { mod wire {} }, Some("a module")),
            (parse_quote! { const LIMIT: u8 = 1; }, None),
        ];
        for (item, shape) in named {
            assert_eq!(item_shape(&item), shape);
        }
    }
}
