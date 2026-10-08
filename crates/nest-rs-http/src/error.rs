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
    /// serde recognised the shape and refused the content. It carries what was
    /// *expected*, never what was read; [`against`](Self::against) names the header.
    Unexpected(Cow<'static, str>),
    /// A type's own refusal (`deserialize_with`, a `Deserialize` impl calling
    /// `custom`); its message may quote the value, so only the fact is carried.
    Refused,
    /// A field naming something that cannot be a header name — the developer's
    /// mistake, surfaced on a request.
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

    /// Attribute a content refusal to the header it was read from: serde's
    /// constructors are static and cannot know it. Idempotent on named variants.
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

    fn missing_field(field: &'static str) -> Self {
        Self::Missing(field.to_owned())
    }

    /// serde's default message opens with the value read off the wire, which
    /// must not be echoed.
    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// One rule over every value-interpolating constructor.
    fn unknown_field(_field: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// serde's default quotes the value in full.
    fn invalid_value(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }

    /// serde's default quotes the value in full.
    fn invalid_type(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }
}

impl std::error::Error for HeaderError {}
