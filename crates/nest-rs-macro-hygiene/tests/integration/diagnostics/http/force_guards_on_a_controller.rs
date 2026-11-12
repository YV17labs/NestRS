//! `#[force_guards]` on a controller struct is refused by name: re-running a
//! guard is a decision about one operation. It reached rustc as `cannot find
//! attribute`.

use nest_rs::http::controller;

struct FreshToken;

#[controller(path = "/widgets")]
#[force_guards(FreshToken)]
struct WidgetsController;

fn main() {}
