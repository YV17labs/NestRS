use uuid::{Uuid, Variant};

use super::parse_uuid::ParseUuid;
use crate::pipe::{Pipe, PipeError};

/// Parse a `String` into an RFC 4122 UUID of an exact `VERSION`. Aliases
/// cover the common ones ([`ParseUuidV4`], [`ParseUuidV7`], …).
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

pub type ParseUuidV3 = ParseUuidVersion<3>;
pub type ParseUuidV4 = ParseUuidVersion<4>;
pub type ParseUuidV5 = ParseUuidVersion<5>;
/// UUID v7 (time-ordered, sortable) — the version nestrs apps mint for ids.
pub type ParseUuidV7 = ParseUuidVersion<7>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_alias_enforces_the_exact_version() {
        let v4 = "550e8400-e29b-41d4-a716-446655440000".to_string();
        assert!(ParseUuidV4::transform(v4.clone()).is_ok());
        assert!(
            ParseUuidV7::transform(v4)
                .unwrap_err()
                .to_string()
                .contains("v7")
        );
    }

    #[test]
    fn accepts_a_freshly_minted_v7() {
        let v7 = Uuid::now_v7().to_string();
        assert!(ParseUuidV7::transform(v7).is_ok());
    }
}
