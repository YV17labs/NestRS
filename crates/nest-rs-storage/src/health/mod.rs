//! The health port's indicator (`health` feature): [`StorageHealthModule`]
//! attaches the store's listing, named `storage`.

mod indicator;
mod module;

pub use module::StorageHealthModule;
