use crate::pipe::{Pipe, PipeError};

/// Lowercase every character of a `String`.
pub struct Lowercase;

impl Pipe for Lowercase {
    type In = String;
    type Out = String;
    fn transform(input: String) -> Result<String, PipeError> {
        Ok(input.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_folds_to_lower() {
        assert_eq!(Lowercase::transform("Aa@X.IO".into()).unwrap(), "aa@x.io");
    }
}
