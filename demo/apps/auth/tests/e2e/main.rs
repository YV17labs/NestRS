//! Postgres-backed e2e for the `auth` issuer — one suite binary,
//! one module per concern, shared boot helpers in `harness`.

mod harness;

mod login;
mod social;
mod token;
