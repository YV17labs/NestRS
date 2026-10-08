//! The one grammar the framework reads an environment boolean with — ungated,
//! since logging reads one before any config exists.

/// `1`/`true`/`yes`/`on` ⇒ `true`, `0`/`false`/`no`/`off` ⇒ `false`, anything
/// else ⇒ `None`. Case-insensitive, trimmed.
pub fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_case_insensitive_and_trimmed() {
        for truthy in ["1", "true", "TRUE", " yes ", "On"] {
            assert_eq!(parse_bool(truthy), Some(true), "{truthy}");
        }
        for falsy in ["0", "false", "FALSE", " no ", "Off"] {
            assert_eq!(parse_bool(falsy), Some(false), "{falsy}");
        }
    }

    #[test]
    fn an_unrecognised_value_is_not_a_no() {
        for other in ["", "y", "enabled", "2", "maybe"] {
            assert_eq!(parse_bool(other), None, "{other}");
        }
    }
}
