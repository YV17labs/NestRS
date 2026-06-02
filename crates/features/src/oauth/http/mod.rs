//! OAuth HTTP adapter — the `/token`, `/authorize`, `/callback`, and
//! `/login` endpoints, plus the OAuth-error → HTTP-status mapping.

mod controller;
mod error;
mod module;

pub use controller::OAuthController;
pub use module::OAuthHttpModule;
