//! Ready-made [`Strategy`](crate::Strategy) implementations, generic over a
//! caller-chosen parameter (claims type, configuration).

mod jwt;

pub use jwt::JwtStrategy;
