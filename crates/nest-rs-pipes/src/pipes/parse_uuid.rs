use uuid::Uuid;

use crate::pipe::{Pipe, PipeError};

/// Parse a `String` into a [`Uuid`] of any version. To require a specific
/// version use [`ParseUuidVersion`](super::parse_uuid_version::ParseUuidVersion)
/// (or an alias like [`ParseUuidV7`](super::parse_uuid_version::ParseUuidV7)).
pub struct ParseUuid;

impl Pipe for ParseUuid {
    type In = String;
    type Out = Uuid;
    fn transform(input: String) -> Result<Uuid, PipeError> {
        Uuid::parse_str(&input).map_err(|_| PipeError::new("must be a valid UUID"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uuid_rejects_non_uuid() {
        assert!(ParseUuid::transform("not-a-uuid".into()).is_err());
    }
}
