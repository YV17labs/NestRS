mod config;
mod module;
mod provider;

pub use config::GithubSocialConfig;
pub use module::{GithubSocialProviderModule, GithubSocialProviderSetup};
pub use provider::GithubSocialProvider;
