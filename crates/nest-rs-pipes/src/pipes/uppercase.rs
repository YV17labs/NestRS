use crate::pipe::{Pipe, PipeError};

/// Full Unicode uppercasing via `str::to_uppercase`: locale-independent, and
/// can change the string's length (e.g. 'ß' → "SS") — not a per-char ASCII fold.
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
    fn case_folds_to_upper() {
        assert_eq!(Uppercase::transform("aa".into()).unwrap(), "AA");
    }
}
