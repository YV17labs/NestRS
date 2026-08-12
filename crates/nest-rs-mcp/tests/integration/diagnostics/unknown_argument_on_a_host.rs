//! The accepted keys, stated by what the decorator refuses.
//!
//! `#[mcp]` takes `path`, `name` and `title` — the mount, and which endpoint
//! stands apart from the app's default. A list of spellings is the right answer
//! for exactly one case: a key that is *nobody's*. Every key that does belong to
//! something — `version`, and every other field of the server's identity — names
//! its owner instead, in the snapshots beside this one. So the argument here is
//! deliberately not one the framework has any home for.

use nest_rs_mcp::mcp;

#[mcp(colour = "puce")]
struct DemoTool;

fn main() {}
