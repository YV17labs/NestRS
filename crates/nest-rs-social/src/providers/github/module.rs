use nest_rs_core::module;

use crate::SocialModule;

/// Opens the GitHub provider: import it beside [`SocialModule`], and the
/// provider activates once `<PREFIX>_SOCIAL__GITHUB__*` is set.
#[module(imports = [SocialModule])]
pub struct GithubSocialModule;
