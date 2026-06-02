use crate::pipe::{Pipe, PipeError};

/// Trim surrounding whitespace from a `String`.
pub struct Trim;

impl Pipe for Trim {
    type In = String;
    type Out = String;
    fn transform(input: String) -> Result<String, PipeError> {
        Ok(input.trim().to_string())
    }
}

/// Lower-case a `String` (e.g. normalise an email before lookup).
pub struct Lowercase;

impl Pipe for Lowercase {
    type In = String;
    type Out = String;
    fn transform(input: String) -> Result<String, PipeError> {
        Ok(input.to_lowercase())
    }
}

/// Upper-case a `String`.
pub struct Uppercase;

impl Pipe for Uppercase {
    type In = String;
    type Out = String;
    fn transform(input: String) -> Result<String, PipeError> {
        Ok(input.to_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_strips_surrounding_whitespace() {
        assert_eq!(Trim::transform("  hi \n".into()).unwrap(), "hi");
    }

    #[test]
    fn case_folds() {
        assert_eq!(Lowercase::transform("Aa@X.IO".into()).unwrap(), "aa@x.io");
        assert_eq!(Uppercase::transform("aa".into()).unwrap(), "AA");
    }
}
