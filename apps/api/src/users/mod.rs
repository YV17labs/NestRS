pub mod dto;
pub mod entity;
pub mod module;
pub mod resolver;
pub mod service;

pub use module::UsersModule;
// `UsersResolver` self-registers at link time, so nothing names it directly.
