use nest_rs::core::{Secret, input};

#[input]
pub struct LoginDto {
    #[validate(email)]
    pub email: String,
    #[validate(length(
        min = 8,
        max = 1024,
        message = "password must be between 8 and 1024 characters"
    ))]
    #[schemars(with = "String")]
    pub password: Secret<String>,
    pub resource: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs::pipes::validator::Validate;

    fn input(email: &str, password: &str) -> LoginDto {
        LoginDto {
            email: email.into(),
            password: password.into(),
            resource: None,
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
    fn a_login_decodes_the_password_it_was_sent() {
        let login: LoginDto =
            serde_json::from_str(r#"{"email":"alice@example.com","password":"longenough"}"#)
                .expect("the login decodes");
        assert_eq!(login.password.expose_secret(), "longenough");
    }

    #[test]
    fn a_serialized_login_holds_no_password() {
        let written = serde_json::to_string(&input("alice@example.com", "longenough"))
            .expect("the login serializes");
        assert!(!written.contains("longenough"), "{written}");
    }

    #[test]
    fn a_refused_password_is_not_named_by_its_refusal() {
        let err = input("alice@example.com", "hunter2")
            .validate()
            .unwrap_err();
        let refusal = serde_json::to_string(&err).expect("the refusal serializes");
        assert!(!refusal.contains("hunter2"), "{refusal}");
    }

    #[test]
    fn exactly_eight_characters_passes() {
        input("alice@example.com", "12345678")
            .validate()
            .expect("8 chars is valid");
    }
}
