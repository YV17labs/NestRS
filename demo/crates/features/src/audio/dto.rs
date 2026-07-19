use nest_rs_http::input;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::ValidationError;

const ALLOWED_AUDIO_EXTENSIONS: [&str; 6] = ["mp3", "wav", "flac", "aac", "ogg", "m4a"];

#[input]
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TranscodeDto {
    #[validate(
        length(min = 1, max = 255),
        custom(function = "validate_transcode_file")
    )]
    pub file: String,
}

#[input]
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UploadRequestDto {
    #[validate(
        length(min = 1, max = 255),
        custom(function = "validate_transcode_file")
    )]
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PresignedUrlDto {
    pub key: String,
    pub url: String,
}

fn validate_transcode_file(file: &str) -> Result<(), ValidationError> {
    if file.contains('/') || file.contains('\\') {
        return Err(ValidationError::new("transcode_file_has_path_separator"));
    }
    if file.contains("..") {
        return Err(ValidationError::new("transcode_file_parent_traversal"));
    }
    let has_allowed_extension = file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && ALLOWED_AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    });
    if !has_allowed_extension {
        return Err(ValidationError::new("transcode_file_unsupported_format"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use validator::Validate;

    use super::*;

    fn dto(file: &str) -> TranscodeDto {
        TranscodeDto { file: file.into() }
    }

    #[test]
    fn plain_audio_filename_passes() {
        dto("podcast-episode_01.mp3").validate().expect("valid");
        dto("song.FLAC")
            .validate()
            .expect("extension is case-insensitive");
    }

    #[test]
    fn parent_directory_traversal_is_rejected() {
        assert!(dto("../../etc/passwd").validate().is_err());
        assert!(dto("..%2fsecret.mp3").validate().is_err());
    }

    #[test]
    fn absolute_and_nested_paths_are_rejected() {
        assert!(dto("/etc/passwd").validate().is_err());
        assert!(dto("uploads/song.mp3").validate().is_err());
        assert!(dto("C:\\Windows\\song.mp3").validate().is_err());
    }

    #[test]
    fn url_like_input_is_rejected() {
        assert!(dto("http://evil.example/song.mp3").validate().is_err());
    }

    #[test]
    fn non_audio_or_missing_extension_is_rejected() {
        assert!(dto("payload.exe").validate().is_err());
        assert!(dto("noextension").validate().is_err());
        assert!(dto(".mp3").validate().is_err());
    }

    #[test]
    fn empty_and_oversized_names_are_rejected() {
        assert!(dto("").validate().is_err());
        let too_long = format!("{}.mp3", "a".repeat(300));
        assert!(dto(&too_long).validate().is_err());
    }
}
