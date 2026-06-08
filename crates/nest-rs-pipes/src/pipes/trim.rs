use crate::pipe::{Pipe, PipeError};

/// Strip surrounding whitespace from a `String`.
pub struct Trim;

impl Pipe for Trim {
    type In = String;
    type Out = String;
    fn transform(input: String) -> Result<String, PipeError> {
        Ok(input.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_strips_surrounding_whitespace() {
        assert_eq!(Trim::transform("  hi \n".into()).unwrap(), "hi");
    }
}
