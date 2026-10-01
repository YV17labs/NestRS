//! Typed errors for the HTTP edge.

use std::borrow::Cow;
use std::fmt;

use crate::header::one_of;

/// What a header a type refused in its own words is said not to be.
const ACCEPTED: &str = "a value its type accepts";

/// What a header binding can fail on. Each variant names the header; none
/// carries its value.
#[derive(Debug)]
pub(crate) enum HeaderError {
    /// A field without a default (i.e. not `Option<_>`) whose header is absent.
    Missing(String),
    /// The header is present but its value is not what the field's type needs.
    Malformed {
        name: String,
        expected: Cow<'static, str>,
    },
    /// serde recognised the shape and refused the content — an enum field whose
    /// text names no variant, a `Deserialize` impl calling `invalid_value`. It
    /// carries what was *expected*, never what was read; the header's name is
    /// put back by [`against`](Self::against), which is the one place that
    /// knows it.
    Unexpected(Cow<'static, str>),
    /// A type's own refusal — a `deserialize_with` function, a custom
    /// `Deserialize` impl calling `custom`. Its message may quote anything, the
    /// value included, so it is carried as the fact alone — the reading
    /// `nest_rs_core::DecodeError` gives the same message — and named against
    /// its header like [`Unexpected`](Self::Unexpected).
    Refused,
    /// A field naming something that cannot be a header name. The developer's
    /// mistake, not the caller's — but it surfaces on a request, so it is
    /// reported the same way and says whose it is.
    NotAHeaderName(String),
}

impl HeaderError {
    pub(crate) fn not_a_header_name(field: &str) -> Self {
        Self::NotAHeaderName(field.to_owned())
    }

    pub(crate) fn malformed(name: &str, expected: impl Into<Cow<'static, str>>) -> Self {
        Self::Malformed {
            name: name.to_owned(),
            expected: expected.into(),
        }
    }

    /// Attribute a content refusal to the header it was read from.
    ///
    /// serde builds `unknown_variant` and friends from **static**
    /// constructors — there is no deserializer in scope to ask which header is
    /// being read — so the name is attached here, by the arm that has one.
    /// Every other variant already names a header, which makes this idempotent
    /// and safe to apply at each arm that hands a value to a visitor.
    pub(crate) fn against(self, name: &str) -> Self {
        match self {
            Self::Unexpected(expected) => Self::malformed(name, expected),
            Self::Refused => Self::malformed(name, ACCEPTED),
            named => named,
        }
    }
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "missing required header `{name}`"),
            Self::Malformed { name, expected } => {
                write!(f, "header `{name}` is not {expected}")
            }
            Self::Unexpected(expected) => write!(f, "header value is not {expected}"),
            Self::Refused => write!(f, "header value is not {ACCEPTED}"),
            Self::NotAHeaderName(field) => write!(
                f,
                "`{field}` is not a valid header name, so no request can carry it — fix the \
                 field's `#[serde(rename = \"…\")]`",
            ),
        }
    }
}

impl serde::de::Error for HeaderError {
    /// The message is dropped unread: it is the type's, and may quote the
    /// header it refused.
    fn custom<T: fmt::Display>(_msg: T) -> Self {
        Self::Refused
    }

    /// serde's derive routes an absent field here, which is what turns
    /// "missing field `X-Request-Id`" into a sentence about headers.
    fn missing_field(field: &'static str) -> Self {
        Self::Missing(field.to_owned())
    }

    /// The one serde constructor a plain header field actually reaches: an
    /// enum-typed field whose text names no variant. The default
    /// (`unknown variant \`{variant}\`, expected …`) opens with the value read
    /// off the wire, which on a header is exactly what must not be echoed.
    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// Same default shape (`unknown field \`{field}\``); a header *name* is not
    /// a secret, but one rule over every value-interpolating constructor is
    /// what stops the next one being missed.
    fn unknown_field(_field: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// `invalid value: string "…", expected …` — one custom `Deserialize` impl
    /// away, and it quotes the value in full.
    fn invalid_value(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }

    /// `invalid type: string "…", expected …`, same reasoning.
    fn invalid_type(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }
}

impl std::error::Error for HeaderError {}
