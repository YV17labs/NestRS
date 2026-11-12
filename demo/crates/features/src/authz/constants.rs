use nest_rs::guards::Scope;

pub const POSTS_READ: Scope = Scope::new("posts:read");
pub const POSTS_WRITE: Scope = Scope::new("posts:write");
pub const AUDIO_TRANSCODE: Scope = Scope::new("audio:transcode");

pub const ALL: [Scope; 3] = [POSTS_READ, POSTS_WRITE, AUDIO_TRANSCODE];

pub fn all() -> Vec<String> {
    ALL.map(String::from).into()
}
