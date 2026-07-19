//! Postgres/Redis/S3-backed e2e for the `api` app — one suite binary,
//! one module per concern, shared boot/token helpers in `harness`.

mod harness;

mod audio;
mod graphql;
mod health;
mod http;
mod openapi;
mod orgs;
mod posts;
mod users;
