use std::marker::PhantomData;
use std::str::FromStr;

use super::parse::short_type_name;
use crate::{PipeError, pipe::Pipe};

/// Split a comma-separated `String` into `Vec<T>`, parsing each item with
/// `T: FromStr` (surrounding whitespace trimmed). Empty input yields an empty
/// `Vec`. A refusal names the item's position and the expected type, never
/// the item.
pub struct ParseArray<T>(PhantomData<fn() -> T>);

impl<T: FromStr> Pipe for ParseArray<T> {
    type In = String;
    type Out = Vec<T>;
    #[expect(
        clippy::map_err_ignore,
        reason = "the refusal is the client's 400: it names the expected shape, never a parser's internals"
    )]
    fn transform(input: String) -> Result<Vec<T>, PipeError> {
        if input.trim().is_empty() {
            return Ok(Vec::new());
        }
        input
            .split(',')
            .enumerate()
            .map(|(index, item)| {
                item.trim().parse::<T>().map_err(|_| {
                    PipeError::new(format!(
                        "item {} must be a valid {}",
                        index + 1,
                        short_type_name::<T>()
                    ))
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

    #[test]
    fn the_refusal_names_the_position_and_the_type() {
        let err = ParseArray::<u32>::transform("1, x ,3".into()).unwrap_err();
        assert_eq!(err.message(), "item 2 must be a valid u32");
    }
}
