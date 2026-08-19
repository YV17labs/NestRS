//! `path` is one key at three host decorators, and until the shared grammar it
//! had three answers: `#[controller]` and `#[gateway]` took the literal and
//! checked nothing, `#[mcp]` refused only the empty string. So a path with a
//! space compiled, mounted, and was logged and documented under an address no
//! client can name — while `version`, declared in the same argument list, has
//! had a shared parser and a bound all along.

use nest_rs_http::controller;

#[controller(path = "/wid gets")]
pub struct WidgetsController;

fn main() {}
