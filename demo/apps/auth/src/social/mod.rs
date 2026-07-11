//! App-local exemplar for keyed / multi-instance providers (two `OAuth2Client`s
//! disambiguated by key). Flattened app-local feature: service + module at the
//! folder root.

mod module;
mod oauth_clients;
mod service;

pub use module::SocialModule;
pub use service::SocialLoginService;
