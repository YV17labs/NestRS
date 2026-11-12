//! Identifier casing helpers shared by decorator macros.

use syn::Ident;

/// `AudioProcessor` → `audio_processor`, `HTTPServer` → `http_server`.
///
/// Camel/Pascal → snake, and **a run of capitals is one word**.
///
/// `OAuth` → `o_auth`: a single capital before a capitalised word cannot be told
/// from a one-letter prefix; spell the type `Oauth`, as Rust spells `Uuid`.
///
/// ```
/// use nest_rs_codegen::snake_case;
///
/// assert_eq!(snake_case("InvoiceAuditor"), "invoice_auditor");
/// assert_eq!(snake_case("HTTPServer"), "http_server");
/// ```
pub fn snake_case(camel: &str) -> String {
    let chars: Vec<char> = camel.chars().collect();
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && i != 0 {
            let after_word = !chars[i - 1].is_uppercase();
            let ends_a_run = chars
                .get(i + 1)
                .is_some_and(|next| next.is_lowercase() || *next == '_');
            if after_word || ends_a_run {
                out.push('_');
            }
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// `org_id` → `OrgId`, keeping the identifier's span: the `<Service>By<Method>`
/// loader struct convention from `#[dataloader]`.
///
/// ```
/// let method: syn::Ident = syn::parse_quote!(by_org_id);
/// assert_eq!(nest_rs_codegen::pascal_case(&method), "ByOrgId");
/// ```
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

/// A method name as a GraphQL schema serves it — a port of `Inflector`'s
/// `to_camel_case`, async-graphql's default (a new word after a digit too):
/// `get_2fa` is `get2Fa`, `userID` stays `userID`. Held against the derive by a test.
pub fn camel_case(name: &str) -> String {
    let trimmed = name.trim_end_matches(|c: char| !c.is_alphanumeric());
    let mut camel = String::with_capacity(trimmed.len());
    let mut new_word = false;
    let mut last = ' ';
    let mut started = false;
    for c in trimmed.chars() {
        if !c.is_alphanumeric() {
            new_word |= started;
        } else if c.is_numeric() {
            started = true;
            new_word = true;
            camel.push(c);
        } else if new_word || (last.is_lowercase() && c.is_uppercase()) {
            started = true;
            new_word = false;
            camel.push(c.to_ascii_uppercase());
        } else {
            started = true;
            last = c;
            camel.push(c.to_ascii_lowercase());
        }
    }
    camel
}

/// Naive English singularization over the last word of a PascalCase name:
/// `Users` → `User`, `Categories` → `Category`, `Statuses` → `Status`; a word
/// already singular passes through. `#[crud]` reads a host's resource by it,
/// and `nestrs g` names an entity by its own copy, which links no framework
/// crate: a test there holds the two alike.
pub fn singular(pascal: &str) -> String {
    let lower = pascal.to_lowercase();
    if let Some(stem) = pascal.get(..pascal.len().saturating_sub(3))
        && lower.ends_with("ies")
    {
        return format!("{stem}y");
    }
    for suffix in ["ses", "xes", "zes", "ches", "shes"] {
        if lower.ends_with(suffix) {
            return pascal[..pascal.len() - 2].to_owned();
        }
    }
    if lower.ends_with("ss") {
        return pascal.to_owned();
    }
    pascal.trim_suffix('s').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plural_reads_as_its_singular_and_a_singular_as_itself() {
        for (plural, singular) in [
            ("Users", "User"),
            ("Categories", "Category"),
            ("Statuses", "Status"),
            ("Boxes", "Box"),
            ("Address", "Address"),
            ("Post", "Post"),
            ("BlogPosts", "BlogPost"),
            ("", ""),
        ] {
            assert_eq!(super::singular(plural), singular, "{plural}");
        }
    }

    #[test]
    fn a_method_name_is_camel_cased_as_async_graphql_does() {
        for (method, served) in [
            ("user_count", "userCount"),
            ("get_2fa", "get2Fa"),
            ("a_1b", "a1B"),
            ("a1_b", "a1B"),
            ("page_2_items", "page2Items"),
            ("v2_api", "v2Api"),
            ("userID", "userID"),
            ("user_id", "userId"),
            ("_leading", "leading"),
            ("trailing_", "trailing"),
            ("x", "x"),
        ] {
            assert_eq!(camel_case(method), served, "{method}");
        }
    }

    #[test]
    fn a_run_of_capitals_is_one_word() {
        assert_eq!(snake_case("AudioProcessor"), "audio_processor");
        assert_eq!(snake_case("HTTPServer"), "http_server");
        assert_eq!(snake_case("APIKey"), "api_key");
        assert_eq!(snake_case("IOHandler"), "io_handler");
        assert_eq!(snake_case("ParseHTTP"), "parse_http");
        assert_eq!(snake_case("HTTP"), "http");
    }

    #[test]
    fn the_ordinary_shapes_are_unchanged() {
        assert_eq!(snake_case("PostsController"), "posts_controller");
        assert_eq!(snake_case("posts"), "posts");
        assert_eq!(snake_case(""), "");
        assert_eq!(snake_case("Transcode2Command"), "transcode2_command");
    }

    #[test]
    fn two_spellings_of_one_acronym_now_reduce_to_one_token() {
        assert_eq!(snake_case("HTTPServer"), snake_case("HttpServer"));
    }

    #[test]
    fn a_single_leading_capital_is_the_shape_no_rule_resolves() {
        assert_eq!(snake_case("OAuth"), "o_auth");
        assert_eq!(snake_case("Oauth"), "oauth");
    }
}
