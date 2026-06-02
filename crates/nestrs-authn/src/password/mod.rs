//! Argon2id password helpers (no DI module — policy lives in `product`).

mod hash;

pub use hash::{burn_verify, hash_password, verify_password, PasswordError};
