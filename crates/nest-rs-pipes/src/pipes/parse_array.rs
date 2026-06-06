use std::marker::PhantomData;
use std::str::FromStr;

use crate::pipe::{Pipe, PipeError};

/// Split a comma-separated `String` into `Vec<T>`, parsing each item with
/// `T: FromStr` (surrounding whitespace trimmed). Empty input yields an empty
/// `Vec`.
pub struct ParseArray<T>(PhantomData<fn() -> T>);

impl<T: FromStr> Pipe for ParseArray<T> {
    type In = String;
    type Out = Vec<T>;
    fn transform(input: String) -> Result<Vec<T>, PipeError> {
        if input.trim().is_empty() {
            return Ok(Vec::new());
        }
        input
            .split(',')
            .map(|item| {
                item.trim().parse::<T>().map_err(|_| {
                    PipeError::new(format!("contains an invalid item: `{}`", item.trim()))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_comma_separated_list() {
        assert_eq!(
            ParseArray::<u32>::transform("1, 2 ,3".into()).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn empty_input_is_an_empty_vec() {
        assert!(
            ParseArray::<u32>::transform("  ".into())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn one_bad_item_rejects_the_whole_list() {
        assert!(ParseArray::<u32>::transform("1,x,3".into()).is_err());
    }
}
