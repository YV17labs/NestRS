use uuid::{Uuid, Variant};

use crate::pipe::{Pipe, PipeError};

/// Parse a `String` into a [`Uuid`] of any version, rejecting a malformed id
/// with a `400`. NestJS's `ParseUUIDPipe` with no `version`; to require a
/// specific version use [`ParseUuidVersion`] (or an alias like [`ParseUuidV7`]).
pub struct ParseUuid;

impl Pipe for ParseUuid {
    type In = String;
    type Out = Uuid;
    fn transform(input: String) -> Result<Uuid, PipeError> {
        Uuid::parse_str(&input).map_err(|_| PipeError::new("must be a valid UUID"))
    }
}

/// Parse a `String` into an RFC 4122 UUID of an exact `VERSION` — NestJS's
/// `ParseUUIDPipe({ version })`, with the version as a const generic so each
/// variant is its own zero-cost type. Aliases cover the common ones
/// ([`ParseUuidV4`], [`ParseUuidV7`], …); write `ParseUuidVersion::<3>` for the
/// rest. Rejects a malformed id, a non-RFC-4122 variant, or the wrong version.
pub struct ParseUuidVersion<const VERSION: u8>;

impl<const VERSION: u8> Pipe for ParseUuidVersion<VERSION> {
    type In = String;
    type Out = Uuid;
    fn transform(input: String) -> Result<Uuid, PipeError> {
        let uuid = ParseUuid::transform(input)?;
        if uuid.get_variant() != Variant::RFC4122 {
            return Err(PipeError::new("must be an RFC 4122 UUID"));
        }
        if uuid.get_version_num() != VERSION as usize {
            return Err(PipeError::new(format!("must be a UUID v{VERSION}")));
        }
        Ok(uuid)
    }
}

/// UUID v3 (name-based, MD5). See [`ParseUuidVersion`].
pub type ParseUuidV3 = ParseUuidVersion<3>;
/// UUID v4 (random). See [`ParseUuidVersion`].
pub type ParseUuidV4 = ParseUuidVersion<4>;
/// UUID v5 (name-based, SHA-1). See [`ParseUuidVersion`].
pub type ParseUuidV5 = ParseUuidVersion<5>;
/// UUID v7 (time-ordered, sortable) — the version nestrs apps mint for ids.
/// See [`ParseUuidVersion`].
pub type ParseUuidV7 = ParseUuidVersion<7>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uuid_rejects_non_uuid() {
        assert!(ParseUuid::transform("not-a-uuid".into()).is_err());
    }

    #[test]
    fn version_alias_enforces_the_exact_version() {
        // A literal v4 (version nibble 4, RFC 4122 variant).
        let v4 = "550e8400-e29b-41d4-a716-446655440000".to_string();
        assert!(ParseUuidV4::transform(v4.clone()).is_ok());
        assert!(ParseUuidV7::transform(v4)
            .unwrap_err()
            .to_string()
            .contains("v7"));
    }

    #[test]
    fn accepts_a_freshly_minted_v7() {
        let v7 = Uuid::now_v7().to_string();
        assert!(ParseUuidV7::transform(v7).is_ok());
    }
}
