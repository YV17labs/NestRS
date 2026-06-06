//! Ready-made [`Strategy`](super::Strategy) implementations. Each is
//! generic over a caller-chosen parameter (claims type, configuration)
//! so apps register the concrete instances they need directly in their own
//! `<Feature>Module`.

mod jwt;

pub use jwt::JwtStrategy;
