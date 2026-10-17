use nest_rs_core::module;

use crate::SocialModule;

/// Opens the Google provider: import it beside [`SocialModule`], and the
/// provider activates once `<PREFIX>_SOCIAL__GOOGLE__*` is set.
#[module(imports = [SocialModule])]
pub struct GoogleSocialModule;
