//! `#[config]` requires a `namespace` — omitting it is a spanned compile error
//! naming the fix, never a silent default that would break `NESTRS_<DOMAIN>__`
//! resolution.

use nest_rs_config::config;

#[config]
struct AppConfig {
    port: u16,
}

fn main() {}
