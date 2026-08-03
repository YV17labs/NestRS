//! The OAuth `scope` claim — one space-delimited string on the wire (RFC 6749
//! §3.3), a list in Rust.
//!
//! Every resource server needs this translation and none of them should write
//! it: getting it wrong silently produces one scope named `"posts:read
//! posts:write"` that matches nothing, and the failure looks like an
//! authorization bug rather than a parsing one.
//!
//! ```rust,ignore
//! #[derive(Serialize, Deserialize)]
//! pub struct Claims {
//!     pub sub: Option<Uuid>,
//!     #[serde(
//!         default,
//!         rename = "scope",
//!         with = "nest_rs::authn::scope::space_delimited",
//!         skip_serializing_if = "Vec::is_empty"
//!     )]
//!     pub scopes: Vec<String>,
//! }
//!
//! impl PrincipalIdentity for Claims {
//!     fn actor_id(&self) -> Option<String> { self.sub.map(|s| s.to_string()) }
//!     fn scopes(&self) -> Option<&[String]> { Some(&self.scopes) }
//! }
//! ```

/// `serde` support for a space-delimited scope list, for
/// `#[serde(with = "...")]`.
pub mod space_delimited {
    use serde::de::Deserializer;
    use serde::{Deserialize, Serialize, Serializer};

    /// Write the list back as the single space-delimited string the wire format
    /// defines.
    pub fn serialize<S: Serializer>(scopes: &[String], serializer: S) -> Result<S::Ok, S::Error> {
        scopes.join(" ").serialize(serializer)
    }

    /// Read the claim into a list.
    ///
    /// Accepts the standard string form *and* a JSON array, because a
    /// deployment does not choose which shape its authorization server emits:
    /// RFC 6749 defines the string, while Entra ID's `roles`, Keycloak's
    /// `realm_access.roles` and several `scp` implementations emit an array.
    /// Refusing the array would reject a valid token for a spelling the client
    /// cannot influence. `null` reads as no scopes.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<String>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Claim {
            Delimited(String),
            List(Vec<String>),
        }

        Ok(match Option::<Claim>::deserialize(deserializer)? {
            None => Vec::new(),
            // `split_whitespace`, not `split(' ')`: a claim padded with a tab or
            // a double space would otherwise yield empty scopes that match
            // nothing and are impossible to spot in a log.
            Some(Claim::Delimited(raw)) => raw.split_whitespace().map(str::to_owned).collect(),
            Some(Claim::List(list)) => list
                .into_iter()
                .filter(|scope| !scope.trim().is_empty())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Claims {
        #[serde(
            default,
            rename = "scope",
            with = "super::space_delimited",
            skip_serializing_if = "Vec::is_empty"
        )]
        scopes: Vec<String>,
    }

    fn parse(json: &str) -> Vec<String> {
        serde_json::from_str::<Claims>(json)
            .expect("the claim parses")
            .scopes
    }

    #[test]
    fn the_standard_string_form_splits_into_scopes() {
        assert_eq!(
            parse(r#"{"scope":"posts:read posts:write"}"#),
            ["posts:read", "posts:write"]
        );
    }

    #[test]
    fn irregular_whitespace_never_yields_an_empty_scope() {
        // A scope of `""` would match nothing and be invisible in a log.
        assert_eq!(
            parse("{\"scope\":\"  posts:read \\t posts:write  \"}"),
            ["posts:read", "posts:write"]
        );
    }

    #[test]
    fn the_array_form_is_accepted_too() {
        // Not RFC 6749's spelling, but one several authorization servers emit —
        // and the deployment does not get to choose.
        assert_eq!(parse(r#"{"scope":["posts:read"]}"#), ["posts:read"]);
    }

    #[test]
    fn an_absent_or_null_claim_is_no_scopes_not_an_error() {
        assert!(parse("{}").is_empty());
        assert!(parse(r#"{"scope":null}"#).is_empty());
    }

    #[test]
    fn the_round_trip_restores_the_wire_form() {
        let claims = Claims {
            scopes: vec!["posts:read".into(), "posts:write".into()],
        };
        let json = serde_json::to_string(&claims).expect("serialize");
        assert_eq!(json, r#"{"scope":"posts:read posts:write"}"#);
        assert_eq!(
            serde_json::from_str::<Claims>(&json).expect("round trip"),
            claims,
        );
    }

    #[test]
    fn an_empty_list_is_omitted_from_the_wire() {
        let json = serde_json::to_string(&Claims { scopes: Vec::new() }).expect("serialize");
        assert_eq!(json, "{}", "an empty `scope` claim carries no information");
    }
}
