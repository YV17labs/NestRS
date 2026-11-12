//! `#[config]` requires a `namespace` — omitting it is a spanned compile error
//! saying how the stem is read off the file's path, with the rule's examples,
//! never one crate's namespace to copy nor a silent default.

use nest_rs::config::config;

#[config]
struct AppConfig {
    port: u16,
}

fn main() {}
