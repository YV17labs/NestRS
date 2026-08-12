//! Identifier casing helpers shared by decorator macros.

use syn::Ident;

/// `AudioProcessor` → `audio_processor`, `HTTPServer` → `http_server`.
///
/// Camel/Pascal → snake, and **a run of capitals is one word**: a `_` goes in
/// before an uppercase letter only where a word actually begins — after a
/// lowercase or a digit, or at the last capital of a run that a lowercase
/// follows. Splitting on every capital instead turned `APIKey` into
/// `a_p_i_key`, which reads as nothing.
///
/// Shared, not copied. Its outputs are Rust identifiers inside macro
/// expansions (`#[processor]`, `#[listeners]`, `#[mcp]` all build one with
/// `format_ident!`) **and** the controller half of an OpenAPI `operationId`,
/// which a client generator turns into a method name — the one output a
/// developer reads. A second implementation would eventually disagree with
/// this one about a name that reaches both.
///
/// One shape no general rule can resolve: a **single** capital followed by a
/// capitalised word, `OAuth` → `o_auth`. Nothing distinguishes it from `XRay`
/// or a genuine one-letter prefix without a dictionary, and a heuristic for it
/// would fire on names it has no business touching. Rust's own convention is
/// the answer — acronyms count as one word (`Uuid`, not `UUID`), so the type
/// is spelled `Oauth`.
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
        // A run that ends the name has no word after it to begin.
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
        // A consequence worth stating rather than discovering: reading a run of
        // capitals as one word makes `HTTPServer` and `HttpServer` the same
        // token, where splitting on every capital kept them apart. Both feed
        // `format_ident!`, so a crate declaring both as processors gets a
        // duplicate-definition compile error — loud, and at the second
        // declaration. Nothing silent turns on it.
        assert_eq!(snake_case("HTTPServer"), snake_case("HttpServer"));
    }

    #[test]
    fn a_single_leading_capital_is_the_shape_no_rule_resolves() {
        // `OAuth` is `O` + `Auth`, and nothing but a dictionary says so — the
        // same input shape as a genuine one-letter prefix. Documented rather
        // than special-cased: Rust's own convention spells the type `Oauth`.
        assert_eq!(snake_case("OAuth"), "o_auth");
        assert_eq!(snake_case("Oauth"), "oauth");
    }
}
