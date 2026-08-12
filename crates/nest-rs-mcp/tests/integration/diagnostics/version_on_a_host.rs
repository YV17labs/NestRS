//! `#[mcp]` takes no `version`, in either shape a developer reaches for it.
//!
//! Alone it is the declaration carried over from `#[controller(version = "1")]`,
//! where it selects an address. Beside a `name` it reads as this endpoint's own
//! `serverInfo.version` — which a host cannot honestly state, since a feature
//! library knows neither the binary's version nor, on a shared endpoint, the
//! whole surface. Both get the same sentence: the address is the whole path, and
//! the server's version is the app's single declaration.
//!
//! The host structs are left undecorated beyond `#[mcp]` on purpose: the
//! argument is refused before anything is emitted, so nothing else can bury the
//! sentence.

use nest_rs_mcp::mcp;

#[mcp(version = "1")]
struct DemoTool;

#[mcp(name = "demo", version = "1")]
struct NamedTool;

fn main() {}
