//! A `#[resource]`'s RFC 6570 template, split into the parts
//! `nest_rs_mcp`'s `UriTemplate` reads a URI back through.
//!
//! Only level-1 expressions (`{name}`) are read: an operator or a modifier
//! (`{+path}`, `{/segments*}`, `{id:3}`) expands to text a URI cannot be read
//! back from by name, and two expressions with no literal between them cannot
//! be told apart.

use syn::LitStr;

/// One piece of a template, in the order it is written.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// Text the URI carries as written.
    Literal(String),
    /// A `{name}` expression.
    Variable(String),
}

/// The characters RFC 6570 §2.2 reserves for an operator, first in an
/// expression.
const OPERATORS: &str = "+#./;?&=,!@|";

/// `template` split into its parts, refused naming what it wrote that no
/// level-1 template does.
pub(crate) fn parse(template: &LitStr) -> syn::Result<Vec<Part>> {
    parts(&template.value()).map_err(|refused| {
        syn::Error::new_spanned(template, format!("#[resource] `template` {refused}"))
    })
}

/// [`parse`] over the template's text, refused in the words after the site.
fn parts(text: &str) -> Result<Vec<Part>, String> {
    if text.is_empty() {
        return Err(
            "names no URI — write the template the resource is read at, e.g. \
             `template = \"post://{id}\"`"
                .to_owned(),
        );
    }
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '{' => {
                let rest = &text[at + 1..];
                let Some(close) = rest.find('}') else {
                    return Err("opens an expression `{` it never closes".to_owned());
                };
                let name = &rest[..close];
                chars.nth(name.chars().count());
                if literal.is_empty() && matches!(parts.last(), Some(Part::Variable(_))) {
                    return Err(format!(
                        "writes `{{{name}}}` right after another expression, so a URI \
                         cannot tell where one value ends — put a literal between them, \
                         e.g. `{{a}}/{{b}}`"
                    ));
                }
                let variable = variable(name)?;
                if parts
                    .iter()
                    .any(|part| matches!(part, Part::Variable(seen) if *seen == variable))
                {
                    return Err(format!(
                        "names `{{{variable}}}` twice — each variable is the one parameter \
                         that takes it"
                    ));
                }
                if !literal.is_empty() {
                    parts.push(Part::Literal(std::mem::take(&mut literal)));
                }
                parts.push(Part::Variable(variable));
            }
            '}' => return Err("closes an expression `}` it never opened".to_owned()),
            '%' => {
                let Some(octet) = text
                    .get(at..at + 3)
                    .filter(|octet| octet[1..].bytes().all(|byte| byte.is_ascii_hexdigit()))
                else {
                    return Err(
                        "writes a `%` that opens no percent-encoded octet (`%XX`)".to_owned()
                    );
                };
                literal.push_str(octet);
                chars.nth(1);
            }
            c if c.is_control()
                || matches!(c, ' ' | '"' | '\'' | '<' | '>' | '\\' | '^' | '`' | '|') =>
            {
                return Err(format!(
                    "writes {c:?}, which a URI never carries as written — percent-encode it"
                ));
            }
            c => literal.push(c),
        }
    }
    if !literal.is_empty() {
        parts.push(Part::Literal(literal));
    }
    Ok(parts)
}

/// The variable a `{…}` expression names, refused when it is no level-1
/// expression or names no parameter.
fn variable(expression: &str) -> Result<String, String> {
    let Some(first) = expression.chars().next() else {
        return Err("holds an empty expression `{}` — name the variable, e.g. `{id}`".to_owned());
    };
    if OPERATORS.contains(first) || expression.contains([':', '*', ',']) {
        return Err(format!(
            "writes `{{{expression}}}`, an RFC 6570 operator, modifier or list: a resource \
             reads each variable back off the URI it is asked for, which only a level-1 \
             expression (`{{name}}`) allows — write one variable, e.g. `{{id}}`"
        ));
    }
    let is_name = (first.is_ascii_alphabetic() || first == '_')
        && expression
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !is_name {
        return Err(format!(
            "names `{{{expression}}}`, which is no parameter's name — a variable is named \
             like the parameter that takes it"
        ));
    }
    Ok(expression.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(text: &str) -> Part {
        Part::Literal(text.to_owned())
    }

    fn variable(name: &str) -> Part {
        Part::Variable(name.to_owned())
    }

    #[test]
    fn a_level_one_template_splits_into_literals_and_variables() {
        assert_eq!(
            parts("post://{id}"),
            Ok(vec![literal("post://"), variable("id")])
        );
        assert_eq!(
            parts("{org}/posts/{id}.json"),
            Ok(vec![
                variable("org"),
                literal("/posts/"),
                variable("id"),
                literal(".json"),
            ])
        );
        assert_eq!(parts("config://app"), Ok(vec![literal("config://app")]));
        assert_eq!(
            parts("file:///a%20b/{name}"),
            Ok(vec![literal("file:///a%20b/"), variable("name")])
        );
        assert_eq!(
            parts("café://{_id2}"),
            Ok(vec![literal("café://"), variable("_id2")])
        );
    }

    #[test]
    fn what_no_uri_can_be_read_back_through_is_refused() {
        for (template, says) in [
            ("", "names no URI"),
            ("post://{id", "never closes"),
            ("post://id}", "never opened"),
            ("post://{}", "empty expression"),
            ("post://{+path}", "operator"),
            ("post://{/segments*}", "operator"),
            ("post://{id:3}", "modifier"),
            ("post://{a,b}", "list"),
            ("post://{1d}", "no parameter's name"),
            ("post://{a-b}", "no parameter's name"),
            ("post://{a}{b}", "right after another expression"),
            ("post://{id}/{id}", "twice"),
            ("post://a b", "percent-encode"),
            ("post://<{id}>", "percent-encode"),
            ("post://%2", "percent-encoded octet"),
            ("post://%zz{id}", "percent-encoded octet"),
        ] {
            let refused = parts(template).expect_err(template);
            assert!(refused.contains(says), "{template}: {refused}");
        }
    }

    #[test]
    fn a_refusal_opens_with_its_site() {
        let template: LitStr = syn::parse_quote!("post://{+path}");
        let refused = parse(&template).expect_err("an operator");
        assert!(
            refused
                .to_string()
                .starts_with("#[resource] `template` writes `{+path}`"),
            "{refused}",
        );
    }
}
