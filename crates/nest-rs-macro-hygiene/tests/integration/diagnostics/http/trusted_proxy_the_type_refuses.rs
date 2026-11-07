//! `trusted_proxy!` given an entry `TrustedProxy` refuses, and given no literal.
//! Each refusal is the boot's own sentence, said as the program compiles; text
//! read at run time is `.parse()?`.

use nest_rs::http::{TrustedProxy, trusted_proxy};

fn main() {
    let past_its_address: TrustedProxy = trusted_proxy!("10.42.0.0/61");
    let every_address: TrustedProxy = trusted_proxy!("0.0.0.0/0");
    let a_name: TrustedProxy = trusted_proxy!("ingress.internal");
    let read: &str = "10.0.0.0/8";
    let not_a_literal: TrustedProxy = trusted_proxy!(read);
    let _ = (past_its_address, every_address, a_name, not_a_literal);
}
