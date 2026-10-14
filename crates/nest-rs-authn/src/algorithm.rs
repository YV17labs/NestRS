//! [`Algorithm`] — the JWS `alg` values this verifier runs.

use std::fmt;
use std::str::FromStr;

use crate::error::UnknownAlgorithmError;

/// A JWS `alg` value this verifier runs (RFC 7518 §3.1, RFC 8037 §3.1), spelled
/// as the RFC spells it.
///
/// Read from `<PREFIX>_AUTHN__ALGORITHMS` by name, case and all, and set in code
/// on [`JwtOptions::algorithms`](crate::JwtOptions::algorithms):
///
/// ```
/// use nest_rs_authn::{Algorithm, JwtOptions};
///
/// let mut options = JwtOptions::jwks("https://issuer.example/jwks");
/// options.algorithms = vec![Algorithm::RS256];
/// assert_eq!("RS256".parse(), Ok(Algorithm::RS256));
/// ```
///
/// It is nestrs's own type, not the JWT library's:
///
/// ```compile_fail
/// let mut options = nest_rs_authn::JwtOptions::jwks("https://issuer.example/jwks");
/// options.algorithms = vec![jsonwebtoken::Algorithm::RS256];
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Algorithm {
    /// HMAC using SHA-256.
    HS256,
    /// HMAC using SHA-384.
    HS384,
    /// HMAC using SHA-512.
    HS512,
    /// RSASSA-PKCS1-v1_5 using SHA-256.
    RS256,
    /// RSASSA-PKCS1-v1_5 using SHA-384.
    RS384,
    /// RSASSA-PKCS1-v1_5 using SHA-512.
    RS512,
    /// RSASSA-PSS using SHA-256 and MGF1 with SHA-256.
    PS256,
    /// RSASSA-PSS using SHA-384 and MGF1 with SHA-384.
    PS384,
    /// RSASSA-PSS using SHA-512 and MGF1 with SHA-512.
    PS512,
    /// ECDSA using P-256 and SHA-256.
    ES256,
    /// ECDSA using P-384 and SHA-384.
    ES384,
    /// EdDSA (RFC 8037), with an Ed25519 key.
    EdDSA,
}

impl Algorithm {
    /// The `alg` value, as RFC 7518 spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HS256 => "HS256",
            Self::HS384 => "HS384",
            Self::HS512 => "HS512",
            Self::RS256 => "RS256",
            Self::RS384 => "RS384",
            Self::RS512 => "RS512",
            Self::PS256 => "PS256",
            Self::PS384 => "PS384",
            Self::PS512 => "PS512",
            Self::ES256 => "ES256",
            Self::ES384 => "ES384",
            Self::EdDSA => "EdDSA",
        }
    }

    /// The key type this algorithm signs and verifies with.
    pub(crate) const fn family(self) -> Family {
        match self {
            Self::HS256 | Self::HS384 | Self::HS512 => Family::Hmac,
            Self::RS256 | Self::RS384 | Self::RS512 | Self::PS256 | Self::PS384 | Self::PS512 => {
                Family::Rsa
            }
            Self::ES256 | Self::ES384 => Family::Ec,
            Self::EdDSA => Family::Ed,
        }
    }

    /// The JWT library's spelling of this algorithm.
    pub(crate) const fn jwt(self) -> jsonwebtoken::Algorithm {
        match self {
            Self::HS256 => jsonwebtoken::Algorithm::HS256,
            Self::HS384 => jsonwebtoken::Algorithm::HS384,
            Self::HS512 => jsonwebtoken::Algorithm::HS512,
            Self::RS256 => jsonwebtoken::Algorithm::RS256,
            Self::RS384 => jsonwebtoken::Algorithm::RS384,
            Self::RS512 => jsonwebtoken::Algorithm::RS512,
            Self::PS256 => jsonwebtoken::Algorithm::PS256,
            Self::PS384 => jsonwebtoken::Algorithm::PS384,
            Self::PS512 => jsonwebtoken::Algorithm::PS512,
            Self::ES256 => jsonwebtoken::Algorithm::ES256,
            Self::ES384 => jsonwebtoken::Algorithm::ES384,
            Self::EdDSA => jsonwebtoken::Algorithm::EdDSA,
        }
    }

    /// The algorithm the JWT library names, or `None` for one this verifier
    /// does not run — the library's enum grows in its minors.
    pub(crate) const fn from_jwt(algorithm: jsonwebtoken::Algorithm) -> Option<Self> {
        Some(match algorithm {
            jsonwebtoken::Algorithm::HS256 => Self::HS256,
            jsonwebtoken::Algorithm::HS384 => Self::HS384,
            jsonwebtoken::Algorithm::HS512 => Self::HS512,
            jsonwebtoken::Algorithm::RS256 => Self::RS256,
            jsonwebtoken::Algorithm::RS384 => Self::RS384,
            jsonwebtoken::Algorithm::RS512 => Self::RS512,
            jsonwebtoken::Algorithm::PS256 => Self::PS256,
            jsonwebtoken::Algorithm::PS384 => Self::PS384,
            jsonwebtoken::Algorithm::PS512 => Self::PS512,
            jsonwebtoken::Algorithm::ES256 => Self::ES256,
            jsonwebtoken::Algorithm::ES384 => Self::ES384,
            jsonwebtoken::Algorithm::EdDSA => Self::EdDSA,
            _ => return None,
        })
    }
}

/// Case-sensitive, as RFC 7515 §4.1.1 reads `alg`; `none` is refused.
impl FromStr for Algorithm {
    type Err = UnknownAlgorithmError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Family::ALL
            .iter()
            .flat_map(|family| family.algorithms())
            .copied()
            .find(|algorithm| algorithm.as_str() == name)
            .ok_or(UnknownAlgorithmError)
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The key type an [`Algorithm`] signs and verifies with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// A shared secret.
    Hmac,
    /// An RSA key.
    Rsa,
    /// An elliptic-curve key, P-256 or P-384.
    Ec,
    /// An Edwards-curve key, Ed25519.
    Ed,
}

impl Family {
    /// Every family, in the order a sentence lists them.
    pub(crate) const ALL: &[Self] = &[Self::Hmac, Self::Rsa, Self::Ec, Self::Ed];

    /// The algorithms of this family, in the order a sentence lists them.
    pub(crate) const fn algorithms(self) -> &'static [Algorithm] {
        match self {
            Self::Hmac => &[Algorithm::HS256, Algorithm::HS384, Algorithm::HS512],
            Self::Rsa => &[
                Algorithm::RS256,
                Algorithm::RS384,
                Algorithm::RS512,
                Algorithm::PS256,
                Algorithm::PS384,
                Algorithm::PS512,
            ],
            Self::Ec => &[Algorithm::ES256, Algorithm::ES384],
            Self::Ed => &[Algorithm::EdDSA],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every() -> impl Iterator<Item = Algorithm> {
        Family::ALL
            .iter()
            .flat_map(|family| family.algorithms())
            .copied()
    }

    #[test]
    fn each_algorithm_is_the_jwt_librarys_and_back() {
        let mut count = 0;
        for algorithm in every() {
            let jwt = algorithm.jwt();
            assert_eq!(Algorithm::from_jwt(jwt), Some(algorithm));
            assert_eq!(format!("{jwt:?}"), algorithm.as_str());
            count += 1;
        }
        assert_eq!(count, 12, "every variant is in one family");
    }

    #[test]
    fn each_algorithm_is_in_the_family_that_lists_it() {
        for family in Family::ALL {
            for algorithm in family.algorithms() {
                assert_eq!(algorithm.family(), *family);
                assert_eq!(algorithm.jwt().family(), jwt_family(*family));
            }
        }
    }

    fn jwt_family(family: Family) -> jsonwebtoken::AlgorithmFamily {
        match family {
            Family::Hmac => jsonwebtoken::AlgorithmFamily::Hmac,
            Family::Rsa => jsonwebtoken::AlgorithmFamily::Rsa,
            Family::Ec => jsonwebtoken::AlgorithmFamily::Ec,
            Family::Ed => jsonwebtoken::AlgorithmFamily::Ed,
        }
    }
}
