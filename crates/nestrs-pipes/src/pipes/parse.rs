use std::marker::PhantomData;
use std::str::FromStr;

use crate::pipe::{Pipe, PipeError};

/// Parse a `String` into any `T: FromStr`. Covers NestJS's
/// `ParseIntPipe`/`ParseFloatPipe`/`ParseBoolPipe` (aliases below) and
/// `ParseEnumPipe` (any enum implementing `FromStr`).
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

pub type ParseInt = Parse<i64>;
pub type ParseFloat = Parse<f64>;
pub type ParseBool = Parse<bool>;

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
