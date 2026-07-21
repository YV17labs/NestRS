//! Integration coverage for `nest-rs-interceptors` — the crate's public API in
//! process, no DB/network. Composition *order* is a wiring property that unit
//! tests on a single interceptor can't show (HTTP-T2).

mod ordering;
