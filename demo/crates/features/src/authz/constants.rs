pub const POSTS_READ: &str = "posts:read";
pub const POSTS_WRITE: &str = "posts:write";
pub const AUDIO_TRANSCODE: &str = "audio:transcode";

pub const ALL: [&str; 3] = [POSTS_READ, POSTS_WRITE, AUDIO_TRANSCODE];

pub fn all() -> Vec<String> {
    ALL.iter().map(|scope| (*scope).to_owned()).collect()
}
