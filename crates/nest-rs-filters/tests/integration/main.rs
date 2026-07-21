//! Integration coverage for `nest-rs-filters` — the crate's public API in
//! process, no DB/network. Which filter maps an error when several are stacked
//! is a wiring property a single-filter unit test can't show (HTTP-T3).

mod ordering;
