//! [`DenialDocument`] — how a route's document states a guard's refusals.

use schemars::{Schema, SchemaGenerator};

/// The HTTP authentication scheme a `401` challenges with, spelled as IANA's
/// HTTP Authentication Scheme Registry spells it: what a route's `security`
/// requires and what its `WWW-Authenticate` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ChallengeScheme {
    /// `Bearer` (RFC 6750): a token.
    Bearer,
    /// `Basic` (RFC 7617): a user-id and a password, a client's at a token
    /// endpoint.
    Basic,
}

impl ChallengeScheme {
    /// The scheme's registered name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer",
            Self::Basic => "Basic",
        }
    }
}

/// How a route's document states a guard's refusals: the scheme it challenges
/// with, and the body it answers when that is no problem document.
///
/// ```
/// use nest_rs_core::{ChallengeScheme, DenialDocument};
///
/// assert_eq!(DenialDocument::PROBLEM.challenge, ChallengeScheme::Bearer);
/// assert!(DenialDocument::PROBLEM.body.is_none());
/// ```
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct DenialDocument {
    /// The scheme a `401` of this guard challenges with.
    pub challenge: ChallengeScheme,
    /// The schema of the `application/json` body its refusals answer in their
    /// protocol's own words; `None` for the problem document a `Denial` renders.
    pub body: Option<fn(&mut SchemaGenerator) -> Schema>,
}

impl DenialDocument {
    /// What every guard refusing through a `Denial` answers: a problem document,
    /// and a `Bearer` challenge on a `401`.
    pub const PROBLEM: Self = Self {
        challenge: ChallengeScheme::Bearer,
        body: None,
    };

    /// A guard answering in its protocol's own words: `challenge` on a `401`,
    /// and `body`'s `application/json` on every status it declares.
    pub const fn answering(
        challenge: ChallengeScheme,
        body: fn(&mut SchemaGenerator) -> Schema,
    ) -> Self {
        Self {
            challenge,
            body: Some(body),
        }
    }
}
