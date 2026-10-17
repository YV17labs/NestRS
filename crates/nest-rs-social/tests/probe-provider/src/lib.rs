//! A third-party social provider package, for `nest-rs-social`'s suites: its
//! one registry entry, key `probe`, activates in every app that imports
//! [`ProbeSocialModule`], since [`ProbeSocialConfig`]'s defaults are complete.

mod config;
mod module;
mod provider;

pub use config::ProbeSocialConfig;
pub use module::ProbeSocialModule;
pub use provider::ProbeSocialProvider;
