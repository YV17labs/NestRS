use nest_rs::core::input;

#[input]
#[derive(Debug, Clone)]
pub struct SendMessageDto {
    pub text: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_message_is_its_text_alone() {
        let parsed: SendMessageDto =
            serde_json::from_value(json!({ "text": "hi" })).expect("a message carrying its text");
        assert_eq!(parsed.text, "hi");
    }

    #[test]
    fn an_author_the_client_names_is_refused_at_the_edge() {
        let impersonated =
            serde_json::from_value::<SendMessageDto>(json!({ "author": "ada", "text": "hi" }));
        assert!(
            impersonated.is_err(),
            "the author is the caller the upgrade authenticated, never a name the payload carries",
        );
    }
}
