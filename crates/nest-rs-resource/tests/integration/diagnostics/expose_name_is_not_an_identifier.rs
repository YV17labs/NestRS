//! `name` declares the wire DTO's type and names its `Create`/`Update` inputs
//! and its OpenAPI schema, so it has to be a Rust identifier. It went straight
//! into `format_ident!`, which panics on anything else: the developer read
//! "proc macro panicked" instead of a sentence naming the key.

use nest_rs_resource::expose;

#[expose(name = "Blog Post")]
pub struct Model {
    pub id: i32,
}

fn main() {}
