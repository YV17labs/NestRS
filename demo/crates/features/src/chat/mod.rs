pub const TARGET: &str = "features::chat";

mod module;
mod posting;
mod service;

pub mod dtos;
pub mod ws;

pub use module::ChatModule;
pub use posting::Posting;
pub use service::ChatService;

pub use ws::ChatWsModule;
