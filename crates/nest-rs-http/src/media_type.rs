//! [`MediaType`]: a `Content-Type` read as RFC 9110 §8.3.1 writes it, its type
//! and subtype at once and its parameters only when asked.

use std::borrow::Cow;

/// Optional whitespace (RFC 9110 §5.6.3).
const OWS: [char; 2] = [' ', '\t'];

/// A `Content-Type`'s type, subtype and parameters: what a body extractor
/// decides on.
pub(crate) struct MediaType<'a> {
    kind: &'a str,
    subtype: &'a str,
    /// Everything from the first `;` on, read by [`parameters`](Self::parameters).
    parameters: &'a str,
}

impl<'a> MediaType<'a> {
    /// Whether this is exactly `kind/subtype`, ignoring case.
    pub(crate) fn is_exactly(&self, kind: &str, subtype: &str) -> bool {
        self.kind.eq_ignore_ascii_case(kind) && self.subtype.eq_ignore_ascii_case(subtype)
    }

    /// Whether this is `kind/subtype` or carries `subtype` as its structured
    /// syntax suffix (`application/problem+json`, RFC 6839 §3.1), ignoring case.
    pub(crate) fn is(&self, kind: &str, subtype: &str) -> bool {
        self.kind.eq_ignore_ascii_case(kind)
            && (self.subtype.eq_ignore_ascii_case(subtype)
                || self
                    .subtype
                    .rsplit_once('+')
                    .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(subtype)))
    }

    /// Its parameters in order, as RFC 9110 §5.6.6 writes them: whitespace
    /// around each `;`, an empty one skipped, a value a token or a quoted-string
    /// handed unquoted; then [`Unparsed`] once, at the first the grammar refuses.
    pub(crate) fn parameters(&self) -> Parameters<'a> {
        Parameters {
            rest: Some(self.parameters),
        }
    }
}

/// `content_type`'s type and subtype, or `None` when it names no `type/subtype`.
pub(crate) fn media_type(content_type: &str) -> Option<MediaType<'_>> {
    let (essence, parameters) =
        content_type.split_at(content_type.find(';').unwrap_or(content_type.len()));
    let (kind, subtype) = essence.trim_matches(OWS).split_once('/')?;
    (!kind.is_empty() && !subtype.is_empty()).then_some(MediaType {
        kind,
        subtype,
        parameters,
    })
}

/// One parameter of a media type: its name as written, its value unquoted.
pub(crate) struct Parameter<'a> {
    pub(crate) name: &'a str,
    pub(crate) value: Cow<'a, str>,
}

/// A parameter list RFC 9110 §5.6.6 does not admit: a name or a value that is
/// no token, a quote left open, a pair with no `=`, anything but `;` after a
/// value.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Unparsed;

/// The parameters [`MediaType::parameters`] reads.
pub(crate) struct Parameters<'a> {
    /// What is left to read; `None` past the end or a refusal.
    rest: Option<&'a str>,
}

impl<'a> Iterator for Parameters<'a> {
    type Item = Result<Parameter<'a>, Unparsed>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut rest = self.rest?;
        let read = loop {
            rest = rest.trim_start_matches(OWS);
            if rest.is_empty() {
                self.rest = None;
                return None;
            }
            let Some(after) = rest.strip_prefix(';') else {
                break Err(Unparsed);
            };
            rest = after.trim_start_matches(OWS);
            if !(rest.is_empty() || rest.starts_with(';')) {
                break parameter(rest);
            }
        };
        // Past a refusal nothing is read.
        self.rest = read.as_ref().ok().map(|&(_, rest)| rest);
        Some(read.map(|(parameter, _)| parameter))
    }
}

/// The parameter `text` starts with, and what follows it.
fn parameter(text: &str) -> Result<(Parameter<'_>, &str), Unparsed> {
    let (name, rest) = text.split_at(token_len(text));
    let rest = rest.strip_prefix('=').ok_or(Unparsed)?;
    if name.is_empty() {
        return Err(Unparsed);
    }
    let (value, rest) = match rest.strip_prefix('"') {
        Some(quoted) => quoted_string(quoted)?,
        None => {
            let (token, rest) = rest.split_at(token_len(rest));
            if token.is_empty() {
                return Err(Unparsed);
            }
            (Cow::Borrowed(token), rest)
        }
    };
    Ok((Parameter { name, value }, rest))
}

/// How many of `text`'s first bytes are `tchar`s (RFC 9110 §5.6.2).
fn token_len(text: &str) -> usize {
    text.bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
        .count()
}

/// The `quoted-string` whose opening quote precedes `quoted`, unquoted, and
/// what follows its closing quote (RFC 9110 §5.6.4): borrowed unless a
/// quoted-pair escapes a character.
fn quoted_string(quoted: &str) -> Result<(Cow<'_, str>, &str), Unparsed> {
    let mut unescaped: Option<String> = None;
    let mut chars = quoted.char_indices();
    while let Some((at, char)) = chars.next() {
        match char {
            '"' => {
                let value = unescaped.map_or(Cow::Borrowed(&quoted[..at]), Cow::Owned);
                return Ok((value, &quoted[at + 1..]));
            }
            '\\' => {
                let (_, escaped) = chars.next().ok_or(Unparsed)?;
                if !(escaped == '\t' || escaped == ' ' || is_visible_or_obs(escaped)) {
                    return Err(Unparsed);
                }
                unescaped
                    .get_or_insert_with(|| quoted[..at].to_owned())
                    .push(escaped);
            }
            '\t' | ' ' | '!' | '#'..='[' | ']'..='~' => {
                if let Some(unescaped) = &mut unescaped {
                    unescaped.push(char);
                }
            }
            char if !char.is_ascii() => {
                if let Some(unescaped) = &mut unescaped {
                    unescaped.push(char);
                }
            }
            _ => return Err(Unparsed),
        }
    }
    Err(Unparsed)
}

/// A `VCHAR` or `obs-text` (RFC 5234 §B.1, RFC 9110 §5.5).
fn is_visible_or_obs(char: char) -> bool {
    char.is_ascii_graphic() || !char.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each parameter of `content_type` as `name=value`, `!` at a refusal.
    fn read(content_type: &str) -> Vec<String> {
        media_type(content_type)
            .expect("a type")
            .parameters()
            .map(|read| match read {
                Ok(Parameter { name, value }) => format!("{name}={value}"),
                Err(Unparsed) => "!".to_owned(),
            })
            .collect()
    }

    #[test]
    fn a_type_names_its_kind_and_subtype_whatever_follows() {
        let media = media_type(" Multipart/Form-Data ; boundary=x").expect("a type");
        assert!(media.is_exactly("multipart", "form-data"));
        assert!(
            media_type("application/problem+json")
                .expect("a type")
                .is("application", "json")
        );
        for unnamed in ["", "multipart", "/form-data", "multipart/", ";boundary=x"] {
            assert!(media_type(unnamed).is_none(), "{unnamed}");
        }
    }

    /// RFC 9110 §5.6.6: `parameters = *( OWS ";" OWS [ parameter ] )`.
    #[test]
    fn whitespace_around_a_separator_and_an_empty_parameter_read_as_nothing() {
        for spelled in [
            "a/b; x=1; y=2",
            "a/b;x=1;y=2",
            "a/b ; x=1 ; y=2 ",
            "a/b\t;\tx=1\t;\ty=2",
            "a/b;; x=1;;; y=2;",
            "a/b; ; x=1 ;; y=2 ; ",
        ] {
            assert_eq!(read(spelled), ["x=1", "y=2"], "{spelled}");
        }
        assert!(read("a/b").is_empty());
        assert!(read("a/b;").is_empty());
        assert!(read("a/b ; ;").is_empty());
    }

    /// RFC 9110 §5.6.4: a quoted-pair stands for the character it escapes.
    #[test]
    fn a_quoted_value_is_handed_unquoted() {
        assert_eq!(read("a/b; x=\"a b;c,d\""), ["x=a b;c,d"]);
        assert_eq!(read("a/b; x=\"\\\"\\\\\\x\""), ["x=\"\\x"]);
        assert_eq!(read("a/b; x=\"\"; y=\"é\""), ["x=", "y=é"]);
        assert!(matches!(
            media_type("a/b; x=\"y\"")
                .expect("a type")
                .parameters()
                .next(),
            Some(Ok(Parameter {
                value: Cow::Borrowed("y"),
                ..
            }))
        ));
    }

    #[test]
    fn what_the_grammar_refuses_ends_the_list_there() {
        for refused in [
            "a/b; x",
            "a/b; x=",
            "a/b; =1",
            "a/b; x =1",
            "a/b; x= 1",
            "a/b; x=1 2",
            "a/b; x=1, y=2",
            "a/b; x=\"1\" 2",
            "a/b; x=\"1",
            "a/b; x=\"1\\",
            "a/b; x=\"1\x7f\"",
            "a/b; x=a\"b\"",
            "a/b; x=[1]",
        ] {
            assert_eq!(
                read(refused).last().map(String::as_str),
                Some("!"),
                "{refused}"
            );
        }
        assert_eq!(read("a/b; x=1; y; z=3"), ["x=1", "!"]);
    }
}
