//! [`RegisteredClaim`] — the claims a JWT access token's specifications
//! register, each held to the JSON type its specification gives it.

use nest_rs_guards::__private::is_scope_token;
use serde_json::{Map, Value};

use crate::error::SignError;

/// A claim RFC 7519 §4.1 registers, or one RFC 9068 §2.2 adds to an access
/// token, with the JSON type its specification gives it and the section saying
/// so.
#[derive(Clone, Copy)]
struct RegisteredClaim {
    name: &'static str,
    shape: Shape,
    specified_by: &'static str,
}

/// The JSON type a registered claim holds.
#[derive(Clone, Copy)]
enum Shape {
    /// A string — a StringOrURI or an identifier.
    String,
    /// RFC 7519 §2's NumericDate: a JSON number of seconds since the epoch.
    NumericDate,
    /// RFC 7519 §4.1.3: one string, or an array of them.
    Audience,
    /// An array of strings.
    Strings,
    /// RFC 6749 §3.3's scope list, one string: scope tokens, each separated by
    /// one space.
    Scope,
}

impl RegisteredClaim {
    /// RFC 7519 §4.1's seven, then RFC 9068 §2.2's: `client_id` and `scope` as
    /// RFC 8693 §4 defines them, `auth_time`, `acr` and `amr` as OpenID Connect
    /// Core 1.0 §2 does.
    const ALL: [Self; 12] = [
        Self::new("iss", Shape::String, "RFC 7519 §4.1.1"),
        Self::new("sub", Shape::String, "RFC 7519 §4.1.2"),
        Self::new("aud", Shape::Audience, "RFC 7519 §4.1.3"),
        Self::new("exp", Shape::NumericDate, "RFC 7519 §4.1.4"),
        Self::new("nbf", Shape::NumericDate, "RFC 7519 §4.1.5"),
        Self::new("iat", Shape::NumericDate, "RFC 7519 §4.1.6"),
        Self::new("jti", Shape::String, "RFC 7519 §4.1.7"),
        Self::new("client_id", Shape::String, "RFC 8693 §4.3"),
        Self::new("scope", Shape::Scope, "RFC 8693 §4.2"),
        Self::new(
            "auth_time",
            Shape::NumericDate,
            "OpenID Connect Core 1.0 §2",
        ),
        Self::new("acr", Shape::String, "OpenID Connect Core 1.0 §2"),
        Self::new("amr", Shape::Strings, "OpenID Connect Core 1.0 §2"),
    ];

    const fn new(name: &'static str, shape: Shape, specified_by: &'static str) -> Self {
        Self {
            name,
            shape,
            specified_by,
        }
    }

    /// Whether `value` is of this claim's JSON type.
    fn holds(self, value: &Value) -> bool {
        match (self.shape, value) {
            (Shape::String | Shape::Audience, Value::String(_)) => true,
            (Shape::NumericDate, Value::Number(seconds)) => {
                seconds.as_f64().is_some_and(|seconds| seconds >= 0.0)
            }
            (Shape::Audience | Shape::Strings, Value::Array(items)) => {
                items.iter().all(Value::is_string)
            }
            (Shape::Scope, Value::String(list)) => {
                list.is_empty() || list.split(' ').all(is_scope_token)
            }
            _ => false,
        }
    }

    /// What the refusal says this claim's JSON type is.
    fn expected(self) -> &'static str {
        match self.shape {
            Shape::String => "string",
            Shape::NumericDate => "NumericDate, a JSON number of seconds since the epoch",
            Shape::Audience => "string or array of strings",
            Shape::Strings => "array of strings",
            Shape::Scope => "scope list, RFC 6749 §3.3's scope tokens each separated by one space",
        }
    }
}

/// `payload` with every registered claim it holds as `null` left out, since a
/// `null` names nothing; refused, naming the claim and quoting nothing, when
/// one holds another JSON type than its specification gives it, which a
/// verifier reading it as that type refuses.
pub(crate) fn held_to_their_types(payload: &mut Map<String, Value>) -> Result<(), SignError> {
    for claim in RegisteredClaim::ALL {
        match payload.get(claim.name) {
            None => {}
            Some(Value::Null) => {
                payload.remove(claim.name);
            }
            Some(value) if claim.holds(value) => {}
            Some(_) => {
                return Err(SignError::new(format!(
                    "the claims' `{}` is no {}: {} makes it one, and a verifier reading one \
                     refuses anything else",
                    claim.name,
                    claim.expected(),
                    claim.specified_by,
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::RegisteredClaim;

    fn named(name: &str) -> RegisteredClaim {
        RegisteredClaim::ALL
            .into_iter()
            .find(|claim| claim.name == name)
            .expect("registered")
    }

    /// RFC 7519 §2's NumericDate counts seconds from the epoch, a fraction
    /// included; the verifier this crate pairs with reads none before it.
    #[test]
    fn a_numeric_date_is_a_number_never_negative() {
        let exp = named("exp");
        for held in [json!(0), json!(1_700_000_000), json!(1.5)] {
            assert!(exp.holds(&held), "{held}");
        }
        for refused in [json!(-1), json!(-0.5), json!("1700000000")] {
            assert!(!exp.holds(&refused), "{refused}");
        }
    }

    /// RFC 6749 §3.3: `scope = scope-token *( SP scope-token )`, so one space
    /// between tokens and none around them; the empty list grants nothing.
    #[test]
    fn a_scope_list_is_the_grammar_or_empty() {
        let scope = named("scope");
        for held in ["", "posts:read", "posts:read posts:write"] {
            assert!(scope.holds(&json!(held)), "{held:?}");
        }
        for refused in [
            " posts:read",
            "posts:read ",
            "a  b",
            "a\tb",
            "a\"b",
            "a\\b",
            "é",
        ] {
            assert!(!scope.holds(&json!(refused)), "{refused:?}");
        }
    }
}
