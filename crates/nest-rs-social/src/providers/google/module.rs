use nest_rs_core::module;

use crate::SocialModule;

/// Opens the Google provider: import it, and the [`SocialModule`] registry it
/// imports activates the provider once `<PREFIX>_SOCIAL__GOOGLE__*` is set.
#[module(imports = [SocialModule])]
pub struct GoogleSocialModule;
