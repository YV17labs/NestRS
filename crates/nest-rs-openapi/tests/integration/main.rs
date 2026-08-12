//! Public-API exercise for `nest-rs-openapi`.
//!
//! The crate's unit tests count the self-mount edges `register` contributes.
//! This suite asks the question those cannot: with the module imported the way
//! the docs say to import it, **what does a caller actually get back** from
//! `/api-json` and `/api` — and what does a caller get when the deployment
//! turned the documentation off.

mod document;
mod module;
