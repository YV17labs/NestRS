use nest_rs_http::input;
use schemars::JsonSchema;
use serde::Serialize;
use validator::ValidationError;

/// Formats the worker is allowed to transcode. An **allowlist** (not a
/// denylist): an unrecognized extension is rejected by default, so a newly
/// added format can't slip through unvetted — fail-secure, matching the
/// framework's opt-in posture.
const ALLOWED_AUDIO_EXTENSIONS: [&str; 6] = ["mp3", "wav", "flac", "aac", "ogg", "m4a"];

/// REST body for `POST /audio/transcode` — a `Dto` (it crosses the HTTP
/// boundary). `#[input]` appends `Deserialize`, `Validate`, and
/// `#[serde(deny_unknown_fields)]`, so the body is validated at the edge (via
/// `Valid<Json<TranscodeDto>>`) before the controller hands `file` to the
/// service, which enqueues a [`super::command::TranscodeCommand`] for the
/// worker. Validating here is what closes the path-traversal / SSRF surface
/// that would otherwise reach the worker over the shared `audio` queue.
#[input]
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TranscodeDto {
    #[validate(
        length(min = 1, max = 255),
        custom(function = "validate_transcode_file")
    )]
    pub file: String,
}

/// Reject anything that is not a bare audio filename: a path separator (`/` or
/// `\`, which also rules out absolute paths and `scheme://` URLs), a
/// parent-directory hop (`..`), or an extension outside
/// [`ALLOWED_AUDIO_EXTENSIONS`]. Runs at the edge, before a `TranscodeCommand`
/// is ever enqueued for the worker.
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
        dto("song.FLAC").validate().expect("extension is case-insensitive");
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
