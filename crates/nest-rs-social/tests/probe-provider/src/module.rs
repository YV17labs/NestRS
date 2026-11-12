use nest_rs::core::module;
use nest_rs::social::SocialModule;

/// Opens the probe provider, as a provider package's module does.
#[module(imports = [SocialModule])]
pub struct ProbeSocialModule;
