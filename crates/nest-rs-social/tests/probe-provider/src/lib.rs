//! A third-party social provider package, for `nest-rs-social`'s suites: its
//! one registry entry, key `probe`, activates in every app that holds it,
//! since [`ProbeSocialConfig`]'s defaults are complete.

mod config;
mod provider;

pub use config::ProbeSocialConfig;
pub use provider::ProbeSocialProvider;
