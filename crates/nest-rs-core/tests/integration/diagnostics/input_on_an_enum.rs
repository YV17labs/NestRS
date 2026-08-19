//! `#[input]` bundles `validator::Validate`, whose derive takes only a struct
//! with named fields — the other three derives accept an enum. The refusal
//! names that fact and the remedy, so a reader can check it and can tell
//! "cannot" from "not yet".

use nest_rs_core::input;

#[input]
pub enum Command {
    Start,
    Stop { after: u32 },
}

fn main() {}
