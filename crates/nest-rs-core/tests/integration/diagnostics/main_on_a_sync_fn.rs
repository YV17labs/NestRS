//! `#[main]` goes on an `async fn`: it runs the function's body on the runtime
//! it builds, and a synchronous body has nothing to run there.

#[nest_rs_core::main]
fn main() {}
