//! Covers `src/pipes/parse_array.rs`.

use nest_rs_pipes::{ParseArray, Pipe};

use crate::{SECRET, carried, quoted_run};

/// A refused item is never quoted back, wherever it sits in the list, and the
/// refusal still says what an item must be.
#[test]
fn a_refused_item_is_never_quoted_back_and_the_expected_type_is_named() {
    for input in [
        SECRET.to_owned(),
        format!("1,{SECRET},3"),
        format!("1, 2,  {SECRET} "),
    ] {
        let refusal = ParseArray::<u64>::transform(input.clone()).unwrap_err();
        let said = carried(&refusal);
        assert_eq!(
            quoted_run(&said, SECRET),
            None,
            "`{input}` is quoted back: {said}"
        );
        assert!(
            refusal.message().contains("u64"),
            "the refusal names what an item must be: {refusal}",
        );
    }
}
