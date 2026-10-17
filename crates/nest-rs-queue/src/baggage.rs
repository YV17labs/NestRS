//! A job's W3C Baggage: the `baggage` list its envelope carried, kept raw for
//! the attempt and written back on every push the attempt makes — carried,
//! never trusted, and never read by the framework.

use std::sync::Arc;

use nest_rs_core::{AmbientValue, Continuation};

/// The list-members W3C Baggage has every platform propagate: a list holding
/// more is dropped whole, never cut.
const MAX_MEMBERS: usize = 64;

/// The bytes W3C Baggage has every platform propagate.
const MAX_BYTES: usize = 8192;

/// A W3C Baggage list a job carried, within the specification's limits, as it
/// was written.
#[derive(PartialEq, Eq)]
pub(crate) struct Baggage(String);

/// Its size, never its members: a producer's baggage may name a user.
impl std::fmt::Debug for Baggage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Baggage({} bytes)", self.0.len())
    }
}

impl Baggage {
    /// `raw` when it is a W3C Baggage list within the limits every platform
    /// propagates; otherwise what is wrong with it, never the value.
    pub(crate) fn parse(raw: &str) -> Result<Self, BaggageRefusal> {
        if raw.len() > MAX_BYTES {
            return Err(BaggageRefusal::Oversized);
        }
        for (index, member) in raw.split(',').enumerate() {
            if index == MAX_MEMBERS {
                return Err(BaggageRefusal::Oversized);
            }
            if !list_member(member.trim_matches(is_ows)) {
                return Err(BaggageRefusal::Malformed);
            }
        }
        Ok(Self(raw.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Propagated wherever the attempt continues, a push included: W3C Baggage has
/// a platform pass on what it does not understand.
impl AmbientValue for Baggage {
    fn carry(self: Arc<Self>, _into: Continuation) -> Option<Arc<Self>> {
        Some(self)
    }
}

/// Why a job's `baggage` was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BaggageRefusal {
    /// Not a list W3C Baggage's grammar reads.
    Malformed,
    /// More list-members or bytes than every platform propagates.
    Oversized,
}

/// `key OWS "=" OWS value *( OWS ";" OWS property )`, its surrounding white
/// space already trimmed.
fn list_member(member: &str) -> bool {
    let mut parts = member.split(';');
    let pair = parts.next().unwrap_or_default();
    pair.split_once('=')
        .is_some_and(|(key, value)| key_value(key, value))
        && parts.all(|property| {
            let property = property.trim_matches(is_ows);
            match property.split_once('=') {
                Some((key, value)) => key_value(key, value),
                None => token(property),
            }
        })
}

/// `key OWS "=" OWS value`, split at its `=`.
fn key_value(key: &str, value: &str) -> bool {
    token(key.trim_end_matches(is_ows)) && value.trim_matches(is_ows).bytes().all(baggage_octet)
}

/// RFC 9110's `token`: one `tchar` or more.
fn token(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// US-ASCII but controls, white space, `"`, `,`, `;` and `\`.
fn baggage_octet(byte: u8) -> bool {
    matches!(byte, 0x21 | 0x23..=0x2B | 0x2D..=0x3A | 0x3C..=0x5B | 0x5D..=0x7E)
}

/// RFC 9110's optional white space.
fn is_ows(c: char) -> bool {
    c == ' ' || c == '\t'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_the_grammar_reads_is_kept_as_it_was_written() {
        for raw in [
            "region=eu",
            "region=eu,plan=pro",
            "region = eu , plan=pro",
            "userId=alice%40example.com;ttl=60;sensitive,serverNode=DF%2028",
            "token=a=b",
            "empty=",
            "k=v ; flag",
        ] {
            let kept = Baggage::parse(raw).unwrap_or_else(|refusal| panic!("{raw:?}: {refusal:?}"));
            assert_eq!(kept.as_str(), raw, "nothing is rewritten");
        }
    }

    #[test]
    fn a_list_the_grammar_refuses_is_malformed() {
        for raw in [
            "",
            "region",
            "=eu",
            "re gion=eu",
            "region=e u",
            "region=\"eu\"",
            "region=eu,",
            ",region=eu",
            "region=eu,,plan=pro",
            "region=é",
            "region=eu;=x",
            "region=eu;",
            "region=eu\\",
        ] {
            assert_eq!(
                Baggage::parse(raw),
                Err(BaggageRefusal::Malformed),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_list_past_what_every_platform_propagates_is_oversized() {
        let members = |count: usize| {
            (0..count)
                .map(|n| format!("k{n}=v"))
                .collect::<Vec<_>>()
                .join(",")
        };
        assert!(Baggage::parse(&members(MAX_MEMBERS)).is_ok());
        assert_eq!(
            Baggage::parse(&members(MAX_MEMBERS + 1)),
            Err(BaggageRefusal::Oversized)
        );

        let at_most = format!("k={}", "v".repeat(MAX_BYTES - 2));
        assert_eq!(at_most.len(), MAX_BYTES);
        assert!(Baggage::parse(&at_most).is_ok());
        assert_eq!(
            Baggage::parse(&format!("{at_most}v")),
            Err(BaggageRefusal::Oversized)
        );
    }
}
