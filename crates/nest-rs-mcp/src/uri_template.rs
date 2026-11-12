//! [`UriTemplate`] — the RFC 6570 template a `#[resource]` reads through, and a
//! URI read back into the values of its variables.
//!
//! `#[tools]` parses the template at expansion and hands over its parts, so the
//! grammar is checked once, where the developer wrote it: level-1 expressions
//! (`{id}`), never two in a row. Reading a URI back is the reverse of RFC 6570
//! §3.2.2's simple expansion: a value is one or more unreserved characters or
//! percent-encoded octets, and ends at the first occurrence of the literal
//! after it.

use std::borrow::Cow;
use std::str::FromStr;

/// One piece of a [`UriTemplate`], in the order the template writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TemplatePart {
    /// Text the URI carries as written.
    Literal(&'static str),
    /// A `{name}` expression, a parameter of the method that reads it.
    Variable(&'static str),
}

/// A URI template `#[resource(template = "…")]` declares, split into its parts
/// by the decorator.
#[derive(Clone, Copy, Debug)]
pub struct UriTemplate {
    template: &'static str,
    parts: &'static [TemplatePart],
}

impl UriTemplate {
    /// `template`, as the decorator split it into `parts`.
    pub const fn new(template: &'static str, parts: &'static [TemplatePart]) -> Self {
        Self { template, parts }
    }

    /// The template as written, which `resources/templates/list` advertises.
    pub fn as_str(&self) -> &'static str {
        self.template
    }

    /// The values `uri` gives this template's variables, in the template's
    /// order; `None` when the template does not expand to `uri`.
    pub fn read<'u>(&self, uri: &'u str) -> Option<TemplateMatch<'u>> {
        let mut rest = uri;
        let mut values = Vec::new();
        for (index, part) in self.parts.iter().enumerate() {
            match part {
                TemplatePart::Literal(literal) => rest = rest.strip_prefix(literal)?,
                TemplatePart::Variable(_) => {
                    let end = match self.parts.get(index + 1) {
                        Some(TemplatePart::Literal(next)) => rest.find(next)?,
                        Some(TemplatePart::Variable(_)) => return None,
                        None => rest.len(),
                    };
                    let (value, after) = rest.split_at(end);
                    if !is_expanded_value(value) {
                        return None;
                    }
                    values.push(value);
                    rest = after;
                }
            }
        }
        rest.is_empty().then_some(TemplateMatch { values })
    }
}

/// The values a URI gives a [`UriTemplate`]'s variables, as the URI carries them.
#[derive(Debug)]
pub struct TemplateMatch<'u> {
    values: Vec<&'u str>,
}

impl TemplateMatch<'_> {
    /// The value of the template's `index`-th variable, percent-decoded and
    /// parsed as `T`; `None` when it is no UTF-8 text or no `T`.
    pub fn variable<T: FromStr>(&self, index: usize) -> Option<T> {
        percent_decoded(self.values.get(index)?)?.parse().ok()
    }
}

/// Whether `value` is what simple expansion writes: unreserved characters and
/// percent-encoded octets (RFC 3986 §2.1, §2.3), one at least.
fn is_expanded_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => match bytes.get(at + 1..at + 3) {
                Some([high, low]) if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {
                    at += 3;
                }
                _ => return false,
            },
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') => {
                at += 1;
            }
            _ => return false,
        }
    }
    !bytes.is_empty()
}

/// `value`, every percent-encoded octet decoded (RFC 3986 §2.1); `None` when the
/// octets are no UTF-8 text. `value` is one [`is_expanded_value`] admitted.
fn percent_decoded(value: &str) -> Option<Cow<'_, str>> {
    if !value.contains('%') {
        return Some(Cow::Borrowed(value));
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok().map(Cow::Owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POST: UriTemplate = UriTemplate::new(
        "post://{id}",
        &[
            TemplatePart::Literal("post://"),
            TemplatePart::Variable("id"),
        ],
    );

    const REVISION: UriTemplate = UriTemplate::new(
        "post://{id}/revisions/{number}.json",
        &[
            TemplatePart::Literal("post://"),
            TemplatePart::Variable("id"),
            TemplatePart::Literal("/revisions/"),
            TemplatePart::Variable("number"),
            TemplatePart::Literal(".json"),
        ],
    );

    #[test]
    fn a_uri_the_template_expands_to_gives_each_variable_its_value() {
        let read = POST.read("post://42").expect("the template expands to it");
        assert_eq!(read.variable::<u32>(0), Some(42));

        let read = REVISION
            .read("post://a7/revisions/3.json")
            .expect("the template expands to it");
        assert_eq!(read.variable::<String>(0).as_deref(), Some("a7"));
        assert_eq!(read.variable::<u8>(1), Some(3));
    }

    #[test]
    fn a_value_ends_at_the_first_occurrence_of_the_literal_after_it() {
        let read = REVISION
            .read("post://a.json/revisions/3.json")
            .expect("`.json` inside the first value is not the closing literal");
        assert_eq!(read.variable::<String>(0).as_deref(), Some("a.json"));
    }

    /// Simple expansion encodes every reserved character, so one in a value
    /// is a URI this template did not write.
    #[test]
    fn a_uri_the_template_does_not_expand_to_is_not_read() {
        for uri in [
            "post://",
            "post://a/b",
            "post://a?b",
            "post://a%2",
            "post://a%zz",
            "page://42",
            "post:/42",
            "post://42/revisions/3.json",
        ] {
            assert!(POST.read(uri).is_none(), "{uri}");
        }
        assert!(REVISION.read("post://a7/revisions/3.jsonx").is_none());
        assert!(REVISION.read("post://a7/revisions/.json").is_none());
    }

    #[test]
    fn a_value_is_percent_decoded_before_it_is_parsed() {
        let read = POST
            .read("post://caf%C3%A9%20au%20lait")
            .expect("encoded octets");
        assert_eq!(read.variable::<String>(0).as_deref(), Some("café au lait"));

        let read = POST.read("post://%FF").expect("an encoded octet");
        assert_eq!(read.variable::<String>(0), None, "no UTF-8 text");
    }

    #[test]
    fn a_value_that_does_not_parse_as_its_type_is_none() {
        let read = POST
            .read("post://not-a-number")
            .expect("the template expands to it");
        assert_eq!(read.variable::<u32>(0), None);
        assert_eq!(
            read.variable::<u32>(1),
            None,
            "the template has one variable"
        );
    }

    #[test]
    fn a_template_without_variables_reads_exactly_its_own_text() {
        const CONFIG: UriTemplate =
            UriTemplate::new("config://app", &[TemplatePart::Literal("config://app")]);
        assert!(CONFIG.read("config://app").is_some());
        assert!(CONFIG.read("config://app/").is_none());
        assert_eq!(CONFIG.as_str(), "config://app");
    }
}
