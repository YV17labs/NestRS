//! The store's indicator (`health` feature): a page of its bucket's listing,
//! named `storage`, which [`StorageModule`](crate::StorageModule) attaches.

mod indicator;

pub(crate) use indicator::register;
