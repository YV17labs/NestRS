use nest_rs::http::input;
use uuid::Uuid;

#[input]
#[derive(Debug, Clone)]
pub struct TranscodeDto {
    pub file: Uuid,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parsed(file: &str) -> Result<TranscodeDto, serde_json::Error> {
        serde_json::from_value(json!({ "file": file }))
    }

    #[test]
    fn a_key_the_upload_minted_parses() {
        let minted = Uuid::now_v7();
        assert_eq!(
            parsed(&minted.to_string()).expect("a minted key").file,
            minted
        );
    }

    #[test]
    fn a_name_the_client_chose_is_refused_at_the_edge() {
        for chosen in [
            "track-1.mp3",
            "../../etc/passwd",
            "uploads/song.mp3",
            "http://evil.example/song.mp3",
            "",
        ] {
            assert!(parsed(chosen).is_err(), "{chosen:?}");
        }
    }
}
