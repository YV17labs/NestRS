use schemars::JsonSchema;
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Deserialize, Validate, JsonSchema)]
pub struct LoginInput {
    #[validate(email)]
    pub email: String,
    #[validate(length(
        min = 8,
        max = 1024,
        message = "password must be between 8 and 1024 characters"
    ))]
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(email: &str, password: &str) -> LoginInput {
        LoginInput {
            email: email.into(),
            password: password.into(),
        }
    }

    #[test]
    fn valid_input_passes() {
        input("alice@example.com", "longenough")
            .validate()
            .expect("valid");
    }

    #[test]
    fn invalid_email_fails() {
        let err = input("no-at-sign", "longenough").validate().unwrap_err();
        assert!(err.field_errors().contains_key("email"));
    }

    #[test]
    fn short_password_fails_with_min_length_message() {
        let err = input("alice@example.com", "short").validate().unwrap_err();
        let fields = err.field_errors();
        let pw = fields.get("password").expect("password error");
        assert!(
            pw.iter().any(|e| e.code == "length"),
            "expected length error, got {pw:?}",
        );
    }

    #[test]
    fn exactly_eight_characters_passes() {
        input("alice@example.com", "12345678")
            .validate()
            .expect("8 chars is valid");
    }
}
