//! Users HTTP adapter — controller + REST error mapping. Importing
//! [`UsersHttpModule`] mounts the controller on the HTTP transport.

mod controller;
mod error;
mod module;

pub use controller::UsersController;
pub use module::UsersHttpModule;
