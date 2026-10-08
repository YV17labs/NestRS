//! Identifier casing helpers shared by decorator macros.

use syn::Ident;

/// `AudioProcessor` → `audio_processor`, `HTTPServer` → `http_server`.
///
/// Camel/Pascal → snake, and **a run of capitals is one word**.
///
/// `OAuth` → `o_auth`: a single capital before a capitalised word cannot be told
/// from a one-letter prefix; spell the type `Oauth`, as Rust spells `Uuid`.
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

#[cfg(test)]
mod tests {
    use super::*;

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
