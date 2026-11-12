use nest_rs_core::module;

use crate::SocialModule;

/// Opens the GitHub provider: import it, and the [`SocialModule`] registry it
/// imports activates the provider once `<PREFIX>_SOCIAL__GITHUB__*` is set.
#[module(imports = [SocialModule])]
pub struct GithubSocialModule;
