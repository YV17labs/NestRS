//! The cross-app authentication contract: the JWT [`Claims`] the `auth` app signs
//! and every resource server verifies, plus the [`Role`] they carry.

mod claims;

pub use claims::{Claims, Role};
