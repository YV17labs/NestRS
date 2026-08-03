//! The OAuth scopes this product defines.
//!
//! One list, three readers, and that is the point: [`AppAbility`] gates its
//! rules on these, the token issuer stamps them, and the deployment advertises
//! them in its RFC 9728 document (`NESTRS_AUTHN__SCOPES_SUPPORTED`). A scope
//! spelled by hand in any one of the three is a scope a client is told to
//! request and can never obtain.
//!
//! [`AppAbility`]: crate::authz::AppAbility

/// Read posts.
pub const POSTS_READ: &str = "posts:read";
/// Create and modify posts.
pub const POSTS_WRITE: &str = "posts:write";
/// Request an audio transcode.
pub const AUDIO_TRANSCODE: &str = "audio:transcode";

/// Every scope this deployment defines — what a **first-party** login carries.
///
/// A password or social login is not a delegated grant: the user is present and
/// authenticating to this product directly, so the token may exercise
/// everything their roles allow. A *delegated* token — the one an MCP client
/// holds — is minted narrower, and that is the case the scope rules exist for.
pub const ALL: [&str; 3] = [POSTS_READ, POSTS_WRITE, AUDIO_TRANSCODE];

/// [`ALL`] as owned strings, for the claim.
pub fn all() -> Vec<String> {
    ALL.iter().map(|scope| (*scope).to_owned()).collect()
}
