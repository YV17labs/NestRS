use std::marker::PhantomData;
use std::str::FromStr;

use crate::pipe::{Pipe, PipeError};

/// Parse a `String` into any `T: FromStr` — the general parse pipe. One generic
/// covers NestJS's `ParseIntPipe` / `ParseFloatPipe` / `ParseBoolPipe` (the
/// aliases below) and `ParseEnumPipe` (any enum implementing `FromStr`, e.g.
/// derived with `strum::EnumString`). Rejects unparseable input with a `400`.
pub struct Parse<T>(PhantomData<fn() -> T>);

impl<T: FromStr> Pipe for Parse<T> {
    type In = String;
    type Out = T;
    fn transform(input: String) -> Result<T, PipeError> {
        input
            .parse::<T>()
            .map_err(|_| PipeError::new(format!("must be a valid {}", short_type_name::<T>())))
    }
}

/// `String` → `i64`. NestJS's `ParseIntPipe`.
pub type ParseInt = Parse<i64>;
/// `String` → `f64`. NestJS's `ParseFloatPipe`.
pub type ParseFloat = Parse<f64>;
/// `String` → `bool`. NestJS's `ParseBoolPipe`.
pub type ParseBool = Parse<bool>;

/// Last path segment of a type name (`app::Color` → `Color`, `i64` → `i64`),
/// for a readable rejection message.
fn short_type_name<T>() -> &'static str {
    let name = std::any::type_name::<T>();
    name.rsplit("::").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_accepts_a_number_and_rejects_text() {
        assert_eq!(ParseInt::transform("42".into()).unwrap(), 42);
        let err = ParseInt::transform("nope".into()).unwrap_err();
        assert!(err.to_string().contains("i64"));
    }

    #[test]
    fn parse_bool_round_trips() {
        assert!(ParseBool::transform("true".into()).unwrap());
        assert!(ParseFloat::transform("1.5".into()).is_ok());
    }
}
