//! [`JwkSet`] — an issuer's JWK Set (RFC 7517 §5), read once per fetch into the
//! keys this verifier may use, each bound to the algorithms it verifies.
//!
//! The binding is the verifier's, never the token's (RFC 8725 §3.1): a key
//! verifies only an algorithm of its key type — RSA the `RS*` and `PS*`
//! families, EC P-256 `ES256`, EC P-384 `ES384`, OKP Ed25519 `EdDSA` — that the
//! verifier accepts, and only the one its `alg` names when it names one. A key
//! whose `use` is not `sig`, or whose `key_ops` leaves out `verify`, is not read;
//! nor is a symmetric (`oct`) key, so no `HS*` algorithm ever verifies against
//! a JWK Set.

use std::sync::Arc;

use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, KeyOperations, PublicKeyUse};
use jsonwebtoken::{Algorithm, AlgorithmFamily, DecodingKey};
use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::error::AuthError;

/// The keys of an issuer's JWK Set this verifier can use.
pub(crate) struct JwkSet {
    keys: Vec<BoundKey>,
    /// How many of the document's keys were not read, for the fetch's line.
    skipped: usize,
}

/// One key, the algorithms it verifies, and its `kid`.
struct BoundKey {
    kid: Option<String>,
    algorithms: Vec<Algorithm>,
    key: Arc<DecodingKey>,
}

/// Which key checks a token, or why none does.
pub(crate) enum Selection {
    /// The one key that checks it.
    Key(Arc<DecodingKey>),
    /// The key the token names is in the set, but bound to other algorithms.
    WrongAlgorithm,
    /// No key fits: the named one is absent, or the token names none and no
    /// key verifies its algorithm — what a refresh may change.
    Unknown,
    /// Several keys fit and the token does not say which.
    Ambiguous,
}

impl Selection {
    /// The key, or the refusal a token gets once no refresh can change it.
    pub(crate) fn into_key(self) -> Result<Arc<DecodingKey>, AuthError> {
        match self {
            Self::Key(key) => Ok(key),
            Self::WrongAlgorithm => Err(AuthError::InvalidAlgorithm),
            Self::Unknown | Self::Ambiguous => Err(AuthError::UnknownKey),
        }
    }
}

/// The document's shape, every entry read on its own: a key this verifier
/// cannot read — a curve it does not run, a member of another type — leaves
/// the others usable.
#[derive(Deserialize)]
struct Document {
    keys: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Key(Box<Jwk>),
    Unread(IgnoredAny),
}

impl JwkSet {
    /// Read `body` as a JWK Set, keeping the keys usable with `accepted`.
    ///
    /// A document that is not a JWK Set is a [`serde_json::Error`], which the
    /// caller reports without quoting the document.
    pub(crate) fn parse(body: &[u8], accepted: &[Algorithm]) -> Result<Self, serde_json::Error> {
        let document: Document = serde_json::from_slice(body)?;
        let read = document.keys.len();
        let keys: Vec<BoundKey> = document
            .keys
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Key(jwk) => bind(&jwk, accepted),
                Entry::Unread(_) => None,
            })
            .collect();
        Ok(Self {
            skipped: read - keys.len(),
            keys,
        })
    }

    /// How many keys this verifier can use.
    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    /// How many of the document's keys it cannot.
    pub(crate) fn skipped(&self) -> usize {
        self.skipped
    }

    /// The key that checks a token signed with `algorithm` under `kid`.
    ///
    /// A token naming no `kid` is checked only when exactly one key of the set
    /// verifies its algorithm: trying each in turn would let a token choose its
    /// key.
    pub(crate) fn select(&self, kid: Option<&str>, algorithm: Algorithm) -> Selection {
        let mut named = self
            .keys
            .iter()
            .filter(|key| kid.is_none_or(|kid| key.kid.as_deref() == Some(kid)))
            .peekable();
        if kid.is_some() && named.peek().is_none() {
            return Selection::Unknown;
        }
        let mut fitting = named.filter(|key| key.algorithms.contains(&algorithm));
        match (fitting.next(), fitting.next()) {
            (Some(key), None) => Selection::Key(Arc::clone(&key.key)),
            (Some(_), Some(_)) => Selection::Ambiguous,
            (None, _) if kid.is_some() => Selection::WrongAlgorithm,
            (None, _) => Selection::Unknown,
        }
    }
}

/// `jwk` as a key this verifier uses, or `None` when it may verify nothing
/// `accepted` holds.
fn bind(jwk: &Jwk, accepted: &[Algorithm]) -> Option<BoundKey> {
    let common = &jwk.common;
    if common
        .public_key_use
        .as_ref()
        .is_some_and(|key_use| *key_use != PublicKeyUse::Signature)
    {
        return None;
    }
    if common
        .key_operations
        .as_ref()
        .is_some_and(|ops| !ops.contains(&KeyOperations::Verify))
    {
        return None;
    }
    let of_its_type: &[Algorithm] = match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => AlgorithmFamily::Rsa.algorithms(),
        AlgorithmParameters::EllipticCurve(params) => match params.curve {
            EllipticCurve::P256 => &[Algorithm::ES256],
            EllipticCurve::P384 => &[Algorithm::ES384],
            _ => &[],
        },
        AlgorithmParameters::OctetKeyPair(params) => match params.curve {
            EllipticCurve::Ed25519 => &[Algorithm::EdDSA],
            _ => &[],
        },
        _ => &[],
    };
    // A key naming an algorithm this verifier does not run (`RSA-OAEP`, a name
    // it does not know) is that algorithm's alone.
    let named = common
        .key_algorithm
        .map(Algorithm::try_from)
        .transpose()
        .ok()?;
    let algorithms: Vec<Algorithm> = of_its_type
        .iter()
        .copied()
        .filter(|algorithm| named.is_none_or(|named| named == *algorithm))
        .filter(|algorithm| accepted.contains(algorithm))
        .collect();
    if algorithms.is_empty() {
        return None;
    }
    let key = DecodingKey::from_jwk(jwk).ok()?;
    Some(BoundKey {
        kid: common.key_id.clone(),
        algorithms,
        key: Arc::new(key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every algorithm a JWK Set may verify.
    fn all() -> Vec<Algorithm> {
        crate::service::algorithms_of(crate::service::JWKS_FAMILIES).collect()
    }

    /// Key material that decodes: selection never runs a signature.
    const B64: &str = "AQAB";

    fn rsa(kid: &str, extra: &str) -> String {
        format!(r#"{{"kty":"RSA","kid":"{kid}","n":"{B64}","e":"{B64}"{extra}}}"#)
    }

    fn ec(kid: &str, crv: &str) -> String {
        format!(r#"{{"kty":"EC","kid":"{kid}","crv":"{crv}","x":"{B64}","y":"{B64}"}}"#)
    }

    fn okp(kid: &str, crv: &str) -> String {
        format!(r#"{{"kty":"OKP","kid":"{kid}","crv":"{crv}","x":"{B64}"}}"#)
    }

    fn set(keys: &[String], accepted: &[Algorithm]) -> JwkSet {
        JwkSet::parse(
            format!(r#"{{"keys":[{}]}}"#, keys.join(",")).as_bytes(),
            accepted,
        )
        .expect("a JWK Set")
    }

    fn selects(set: &JwkSet, kid: Option<&str>, algorithm: Algorithm) -> &'static str {
        match set.select(kid, algorithm) {
            Selection::Key(_) => "key",
            Selection::WrongAlgorithm => "wrong algorithm",
            Selection::Unknown => "unknown",
            Selection::Ambiguous => "ambiguous",
        }
    }

    #[test]
    fn a_key_verifies_only_the_algorithms_of_its_type() {
        let set = set(
            &[
                rsa("r", ""),
                ec("p256", "P-256"),
                ec("p384", "P-384"),
                okp("ed", "Ed25519"),
            ],
            &all(),
        );
        for (kid, verifies) in [
            ("r", AlgorithmFamily::Rsa.algorithms()),
            ("p256", &[Algorithm::ES256][..]),
            ("p384", &[Algorithm::ES384][..]),
            ("ed", &[Algorithm::EdDSA][..]),
        ] {
            for algorithm in &all() {
                let expected = if verifies.contains(algorithm) {
                    "key"
                } else {
                    "wrong algorithm"
                };
                assert_eq!(
                    selects(&set, Some(kid), *algorithm),
                    expected,
                    "{kid} under {algorithm:?}"
                );
            }
        }
    }

    #[test]
    fn a_key_naming_its_algorithm_verifies_that_one_alone() {
        let set = set(&[rsa("r", r#","alg":"PS256""#)], &all());
        assert_eq!(selects(&set, Some("r"), Algorithm::PS256), "key");
        assert_eq!(
            selects(&set, Some("r"), Algorithm::RS256),
            "wrong algorithm"
        );
    }

    #[test]
    fn a_key_naming_an_algorithm_of_another_type_is_not_read() {
        let set = set(
            &[ec("e", "P-256").replace('}', r#","alg":"RS256"}"#)],
            &all(),
        );
        assert_eq!(set.len(), 0);
        assert_eq!(set.skipped(), 1);
    }

    #[test]
    fn the_accepted_algorithms_narrow_what_every_key_verifies() {
        let set = set(&[rsa("r", ""), okp("ed", "Ed25519")], &[Algorithm::RS256]);
        assert_eq!(selects(&set, Some("r"), Algorithm::RS256), "key");
        assert_eq!(
            selects(&set, Some("r"), Algorithm::PS256),
            "wrong algorithm"
        );
        assert_eq!(set.len(), 1, "the Ed25519 key verifies nothing accepted");
        assert_eq!(selects(&set, Some("ed"), Algorithm::EdDSA), "unknown");
    }

    #[test]
    fn a_key_not_for_signatures_is_not_read() {
        let set = set(
            &[
                rsa("enc", r#","use":"enc""#),
                rsa("wrap", r#","key_ops":["wrapKey"]"#),
                rsa("oaep", r#","alg":"RSA-OAEP""#),
                rsa("future", r#","alg":"ML-DSA-65""#),
                rsa("sig", r#","use":"sig","key_ops":["verify"]"#),
            ],
            &all(),
        );
        assert_eq!((set.len(), set.skipped()), (1, 4));
        for kid in ["enc", "wrap", "oaep", "future"] {
            assert_eq!(
                selects(&set, Some(kid), Algorithm::RS256),
                "unknown",
                "{kid}"
            );
        }
        assert_eq!(selects(&set, Some("sig"), Algorithm::RS256), "key");
    }

    #[test]
    fn a_symmetric_key_or_a_curve_not_run_here_is_not_read() {
        let set = set(
            &[
                format!(r#"{{"kty":"oct","kid":"hs","k":"{B64}"}}"#),
                ec("p521", "P-521"),
                okp("x", "X25519"),
                okp("ed448", "Ed448"),
                r#"{"kty":"EC","use":7}"#.to_owned(),
            ],
            &all(),
        );
        assert_eq!((set.len(), set.skipped()), (0, 5));
    }

    #[test]
    fn a_token_without_a_kid_is_checked_only_by_the_one_key_that_fits() {
        let one = set(&[rsa("r", ""), okp("ed", "Ed25519")], &all());
        assert_eq!(selects(&one, None, Algorithm::EdDSA), "key");
        assert_eq!(selects(&one, None, Algorithm::ES256), "unknown");

        let two = set(&[okp("ed-1", "Ed25519"), okp("ed-2", "Ed25519")], &all());
        assert_eq!(selects(&two, None, Algorithm::EdDSA), "ambiguous");
        assert_eq!(selects(&two, Some("ed-2"), Algorithm::EdDSA), "key");
    }

    #[test]
    fn two_keys_under_one_kid_are_told_apart_by_algorithm_or_refused() {
        let set = set(
            &[
                rsa("k", ""),
                ec("k", "P-256"),
                ec("dup", "P-256"),
                ec("dup", "P-256"),
            ],
            &all(),
        );
        assert_eq!(selects(&set, Some("k"), Algorithm::ES256), "key");
        assert_eq!(selects(&set, Some("k"), Algorithm::PS384), "key");
        assert_eq!(selects(&set, Some("dup"), Algorithm::ES256), "ambiguous");
    }

    #[test]
    fn a_document_that_is_not_a_jwk_set_does_not_parse() {
        for body in [&b"[]"[..], b"{}", br#"{"keys":{}}"#, b"not json"] {
            assert!(JwkSet::parse(body, &all()).is_err());
        }
        assert_eq!(
            JwkSet::parse(br#"{"keys":[]}"#, &all())
                .map(|set| set.len())
                .ok(),
            Some(0)
        );
    }
}
