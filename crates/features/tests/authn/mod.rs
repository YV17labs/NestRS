//! `features::authn` is alias-only — it re-binds `nestrs_authn::AuthGuard<JwtStrategy<Claims>>`
//! as the project-wide `AuthGuard` and registers the verification side of
//! `JwtService`. There is no runtime logic to assert here in isolation.
//!
//! Behaviour coverage:
//! - `apps/api` and `apps/auth` e2e exercise the full bearer-JWT round trip.
//! - `crates/nestrs-authn/tests/jwt/` covers the underlying `JwtService` and
//!   `JwtStrategy<C>` machinery.
